//! DICOM files as Privacy Mode sends them (docs/security.md, "Files"): the header rewritten,
//! the pixel data copied byte for byte.
//!
//! The rewrite applies the actions of DICOM PS3.15 Table E.1-1 (the Basic Application Level
//! Confidentiality Profile) for the attributes listed in `PROFILE`, with the "Retain
//! Longitudinal Temporal Information with Modified Dates" option (the catalog's offset and the
//! patient's own days, so the patient's intervals are kept).  It is not a certified
//! implementation of the profile: what the table names and this list does not is kept, its
//! text scrubbed (below).
//!
//! | Element | Becomes |
//! |---|---|
//! | Patient's Name | `[Patient Name]` |
//! | Patient ID | the patient's pseudonym |
//! | Patient's Birth Date | empty |
//! | Patient's Age | capped at 90 years (as `090Y`) when the settings cap ages |
//! | Patient's Sex, Size, Weight | kept |
//! | every other element of the patient group (0010) | removed (other IDs and names, address, phone, comments …) |
//! | the clinical trial module (0012: subject, site, sponsor, protocol) | removed |
//! | other person names (VR PN: physicians, operators, observers) | `[Staff Name]` |
//! | the attributes of `PROFILE`: institution, department, station, addresses and phones, device and detector IDs, accession, study, order, procedure, admission and visit IDs, descriptions (study, series, procedure, derivation, diagnoses), comments, protocol names, SR text and content (Text Value, Content Sequence), graphic annotations, icons, digital signatures | removed (X), or emptied (Z) where the table says so |
//! | curves (50xx), overlays (60xx), results (4008) | removed |
//! | private elements (odd groups) | removed |
//! | the free text left (VR SH, LO, ST, LT, UC, UT; in implicit-VR files, elements of unknown VR that hold text) | the patient's names, date of birth and identifiers replaced, dates in it moved (gate.rs `Restricted::free_text`) |
//! | instance UIDs (study, series, instance, frame of reference, references) | replaced: `2.25.` and a 128-bit keyed hash of the original (HMAC-SHA-256 with the catalog's secret), so a series still holds together across requests, and nobody without the secret can map them back |
//! | dates and times (DA, DT, TM) | moved back by the offset; a date and its time together |
//! | Patient Identity Removed, De-identification Method, Longitudinal Temporal Information Modified | added |
//!
//! Sequences are rewritten with undefined lengths (allowed by the standard), so that changed
//! lengths never need to be propagated upwards; sequences nested more than `MAX_DEPTH` deep
//! are refused.  The file keeps its transfer syntax; deflated and big-endian files are refused
//! (they could not be rewritten safely).  What the pixels show is not changed: burned-in text
//! (ultrasound, secondary captures) may still name the patient, so these files always go out
//! with the "may contain PHI" warning.

use std::io::Read;

use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub const IMPLICIT: &str = "1.2.840.10008.1.2";
const BIG_ENDIAN: &str = "1.2.840.10008.1.2.2";
const DEFLATED: &str = "1.2.840.10008.1.2.1.99";
const PIXEL_DATA: u32 = 0x7FE0_0010;
const ITEM: u32 = 0xFFFE_E000;
const ITEM_END: u32 = 0xFFFE_E00D;
const SEQ_END: u32 = 0xFFFE_E0DD;
const UNDEFINED: u32 = 0xFFFF_FFFF;

/// What the rewrite needs to know.
pub struct Rewrite<'a> {
    /// The patient's pseudonym (Patient ID).
    pub pseudonym: &'a str,
    /// Dates move back by this many seconds (0: kept).
    pub offset_s: i64,
    pub cap_ages: bool,
    /// The catalog's secret, for UIDs.
    pub uid_key: &'a [u8],
    /// Free text as Privacy Mode writes it (the values of text VRs left by the profile, and of
    /// elements of unknown VR that hold text, in implicit-VR files): the patient's names and
    /// identifiers replaced, dates moved (gate.rs `Restricted::free_text`).
    pub text: &'a dyn Fn(&str) -> String,
}

/// A rewritten file: the new bytes of everything before the pixel data, and where the rest of
/// the original (the pixel data element and what follows it) starts.
pub struct Rewritten {
    pub head: Vec<u8>,
    pub rest_from: u64,
    /// The file says its pixels hold burned-in text (Burned In Annotation = YES).
    pub burned_in: bool,
}

/// [`deidentify`] of a whole file held in memory (the fuzzer's and the tests' form).
pub fn deidentify_bytes(buf: &[u8], r: &Rewrite) -> anyhow::Result<Rewritten> {
    match rewrite(buf, r, true) {
        Ok(out) => Ok(out),
        Err(Error::NeedMore) => anyhow::bail!("the DICOM file ends inside an element"),
        Err(Error::Bad(e)) => anyhow::bail!(e),
    }
}

/// Rewrites a DICOM file (Part 10, or a bare dataset in implicit VR little endian).
pub fn deidentify(path: &std::path::Path, r: &Rewrite) -> anyhow::Result<Rewritten> {
    let size = std::fs::metadata(path)?.len();
    // headers are small: try the first megabytes, and the whole file when they hold no pixels
    for limit in [4u64 << 20, size] {
        let mut buf = vec![];
        std::fs::File::open(path)?.take(limit.min(size)).read_to_end(&mut buf)?;
        match rewrite(&buf, r, limit >= size) {
            Ok(out) => return Ok(out),
            Err(Error::NeedMore) if limit < size => continue,
            Err(Error::NeedMore) => anyhow::bail!("the DICOM file ends inside an element"),
            Err(Error::Bad(e)) => anyhow::bail!(e),
        }
    }
    anyhow::bail!("the DICOM file ends inside an element")
}

enum Error {
    NeedMore,
    Bad(String),
}

fn bad<T>(msg: impl Into<String>) -> Result<T, Error> {
    Err(Error::Bad(msg.into()))
}

/// The dataset's encoding.
#[derive(Clone, Copy)]
struct Enc {
    explicit: bool,
}

/// One element as read: tag, VR (explicit files; else from the tags known here), value bytes or
/// items, where it started.
struct Element {
    tag: u32,
    vr: [u8; 2],
    value: Value,
}

enum Value {
    Bytes(Vec<u8>),
    /// A sequence's items, each a list of elements.
    Items(Vec<Vec<Element>>),
}

const LONG_VR: [&[u8; 2]; 13] =
    [b"OB", b"OD", b"OF", b"OL", b"OV", b"OW", b"SQ", b"SV", b"UC", b"UN", b"UR", b"UT", b"UV"];

fn le16(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|x| u16::from_le_bytes([x[0], x[1]]))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]))
}

fn tag_at(b: &[u8], at: usize) -> Option<u32> {
    Some(((le16(b, at)? as u32) << 16) | le16(b, at + 2)? as u32)
}

/// Reads elements from `pos` until `end` (or a delimiter, or the pixel data at the top level).
/// Returns the elements, the position after them, and whether it stopped at the pixel data.
/// `depth`: the sequences it is in (at most [`MAX_DEPTH`]: a crafted file must not exhaust the
/// stack; [`clean`] and [`write_elements`] recurse on what is read here, so no deeper).
fn read_elements(
    b: &[u8],
    mut pos: usize,
    end: Option<usize>,
    enc: Enc,
    top: bool,
    complete: bool,
    depth: usize,
) -> Result<(Vec<Element>, usize, bool), Error> {
    if depth > MAX_DEPTH {
        return bad(format!("sequences nested more than {MAX_DEPTH} deep"));
    }
    let mut out = vec![];
    loop {
        if end.is_some_and(|e| pos >= e) {
            return Ok((out, pos, false));
        }
        if pos >= b.len() {
            if end.is_none() && top && complete {
                return Ok((out, pos, false)); // a dataset without pixel data, read to its end
            }
            return Err(Error::NeedMore);
        }
        let Some(tag) = tag_at(b, pos) else { return Err(Error::NeedMore) };
        if tag == ITEM_END || tag == SEQ_END {
            return Ok((out, pos, false)); // the caller reads the delimiter
        }
        if top && tag == PIXEL_DATA {
            return Ok((out, pos, true));
        }
        let (vr, len, hdr) = if enc.explicit && (tag >> 16) != 0xFFFE {
            let vr: [u8; 2] = b.get(pos + 4..pos + 6).ok_or(Error::NeedMore)?.try_into().unwrap();
            if !vr.iter().all(u8::is_ascii_uppercase) {
                return bad(format!("not a DICOM element at byte {pos}"));
            }
            if LONG_VR.contains(&&vr) {
                (vr, le32(b, pos + 8).ok_or(Error::NeedMore)?, 12)
            } else {
                (vr, le16(b, pos + 6).ok_or(Error::NeedMore)? as u32, 8)
            }
        } else {
            (known_vr(tag), le32(b, pos + 4).ok_or(Error::NeedMore)?, 8)
        };
        let start = pos + hdr;
        let is_seq = &vr == b"SQ"
            || (len == UNDEFINED && tag != PIXEL_DATA)
            || (!enc.explicit && len != UNDEFINED && len >= 8 && tag_at(b, start) == Some(ITEM));
        if is_seq {
            // UN of undefined length: a sequence encoded in implicit VR (PS3.5 6.2.2)
            let enc = if &vr == b"UN" { Enc { explicit: false } } else { enc };
            let mut items = vec![];
            let stop = (len != UNDEFINED).then(|| start + len as usize);
            let mut p = start;
            loop {
                if stop.is_some_and(|s| p >= s) {
                    break;
                }
                let Some(t) = tag_at(b, p) else { return Err(Error::NeedMore) };
                if t == SEQ_END {
                    p += 8;
                    break;
                }
                if t != ITEM {
                    return bad(format!("a sequence holds something else than items at byte {p}"));
                }
                let ilen = le32(b, p + 4).ok_or(Error::NeedMore)?;
                let istart = p + 8;
                let iend = (ilen != UNDEFINED).then(|| istart + ilen as usize);
                let (els, after, _) = read_elements(b, istart, iend, enc, false, complete, depth + 1)?;
                p = after;
                if ilen == UNDEFINED {
                    if tag_at(b, p) != Some(ITEM_END) {
                        return Err(Error::NeedMore);
                    }
                    p += 8;
                }
                items.push(els);
            }
            out.push(Element { tag, vr: *b"SQ", value: Value::Items(items) });
            pos = p;
            continue;
        }
        if len == UNDEFINED {
            return bad(format!("an element of undefined length at byte {pos}"));
        }
        let stop = start + len as usize;
        let v = b.get(start..stop).ok_or(Error::NeedMore)?.to_vec();
        out.push(Element { tag, vr, value: Value::Bytes(v) });
        pos = stop;
    }
}

/// VRs of the elements this rewrite changes, for files in implicit VR.
fn known_vr(tag: u32) -> [u8; 2] {
    match tag {
        0x0010_0010 | 0x0008_0090 | 0x0008_009C | 0x0008_1048 | 0x0008_1050 | 0x0008_1060 | 0x0008_1070
        | 0x0032_1032 | 0x0040_0006 | 0x0040_2008 | 0x0040_4037 | 0x0040_A075 | 0x0040_A123 | 0x0040_1010
        | 0x0070_0084 | 0x4008_0114 | 0x0010_1001 | 0x0010_1005 | 0x0010_1060 | 0x0010_2297 | 0x3006_00A6
        | 0x300E_0008 => *b"PN",
        0x0020_000D | 0x0020_000E | 0x0008_0018 | 0x0020_0052 | 0x0008_1155 | 0x0002_0003 | 0x0008_0014
        | 0x0020_0200 | 0x0040_A124 | 0x0008_3010 | 0x0020_9161 | 0x0004_1511 | 0x3006_0024 | 0x0008_0016
        | 0x0008_1150 | 0x0002_0010 | 0x0002_0002 => *b"UI",
        0x0008_0012 | 0x0008_0020 | 0x0008_0021 | 0x0008_0022 | 0x0008_0023 | 0x0010_0030 | 0x0040_0244
        | 0x0040_0250 | 0x0040_A121 | 0x0018_1012 | 0x0018_1200 | 0x0032_1050 | 0x0040_2004 => *b"DA",
        0x0008_0013 | 0x0008_0030 | 0x0008_0031 | 0x0008_0032 | 0x0008_0033 | 0x0040_0245 | 0x0040_0251
        | 0x0040_A122 | 0x0018_1014 | 0x0018_1201 | 0x0040_2005 => *b"TM",
        0x0008_002A | 0x0040_A032 | 0x0040_A120 => *b"DT",
        0x0010_1010 => *b"AS",
        0x0010_0020 => *b"LO",
        0x0028_0301 => *b"CS",
        _ => *b"UN",
    }
}

/// Date-and-time pairs moved together.
const PAIRS: &[(u32, u32)] = &[
    (0x0008_0012, 0x0008_0013),
    (0x0008_0020, 0x0008_0030),
    (0x0008_0021, 0x0008_0031),
    (0x0008_0022, 0x0008_0032),
    (0x0008_0023, 0x0008_0033),
    (0x0040_0244, 0x0040_0245),
    (0x0040_0250, 0x0040_0251),
    (0x0040_A121, 0x0040_A122),
    (0x0018_1012, 0x0018_1014),
    (0x0018_1200, 0x0018_1201),
    (0x0040_2004, 0x0040_2005),
];

/// What the rewrite does with an attribute of PS3.15 Table E.1-1 (the Basic Profile), wherever
/// it is: `X` removed, `Z` kept empty.  The profile's other actions are general rules here: `D`
/// for person names (`[Staff Name]`), `U` for UIDs, and dates moved back (the "Modified Dates"
/// option) instead of removed.  Reference sequences are kept, their UIDs replaced.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    X,
    Z,
}

/// The attributes of Table E.1-1 the rewrite removes or empties (the patient group, 0010, and
/// the clinical trial group, 0012, are handled as groups: [`clean`]).
const PROFILE: &[(u32, Action)] = {
    use Action::{X, Z};
    &[
        (0x0002_0016, X), // Source Application Entity Title
        (0x0002_0100, X), // Private Information Creator UID
        (0x0002_0102, X), // Private Information
        (0x0008_0015, X), // Instance Coercion DateTime
        (0x0008_0050, Z), // Accession Number
        (0x0008_0080, Z), // Institution Name
        (0x0008_0081, X), // Institution Address
        (0x0008_0082, X), // Institution Code Sequence
        (0x0008_0092, X), // Referring Physician's Address
        (0x0008_0094, X), // Referring Physician's Telephone Numbers
        (0x0008_0096, X), // Referring Physician Identification Sequence
        (0x0008_009D, X), // Consulting Physician Identification Sequence
        (0x0008_0201, X), // Timezone Offset From UTC
        (0x0008_1010, X), // Station Name
        (0x0008_1030, X), // Study Description
        (0x0008_1032, X), // Procedure Code Sequence
        (0x0008_103E, X), // Series Description
        (0x0008_1040, X), // Institutional Department Name
        (0x0008_1041, X), // Institutional Department Type Code Sequence
        (0x0008_1048, X), // Physician(s) of Record
        (0x0008_1049, X), // Physician(s) of Record Identification Sequence
        (0x0008_1050, X), // Performing Physicians' Name
        (0x0008_1052, X), // Performing Physician Identification Sequence
        (0x0008_1060, X), // Name of Physician(s) Reading Study
        (0x0008_1062, X), // Physician(s) Reading Study Identification Sequence
        (0x0008_1072, X), // Operator Identification Sequence
        (0x0008_1080, X), // Admitting Diagnoses Description
        (0x0008_1084, X), // Admitting Diagnoses Code Sequence
        (0x0008_1111, X), // Referenced Performed Procedure Step Sequence
        (0x0008_1120, X), // Referenced Patient Sequence
        (0x0008_2111, X), // Derivation Description
        (0x0008_4000, X), // Identifying Comments
        (0x0018_1000, X), // Device Serial Number
        (0x0018_1004, X), // Plate ID
        (0x0018_1005, X), // Generator ID
        (0x0018_1007, X), // Cassette ID
        (0x0018_1008, X), // Gantry ID
        (0x0018_1009, X), // Unique Device Identifier
        (0x0018_100A, X), // UDI Sequence
        (0x0018_1030, X), // Protocol Name
        (0x0018_1400, X), // Acquisition Device Processing Description
        (0x0018_4000, X), // Acquisition Comments
        (0x0018_700A, X), // Detector ID
        (0x0018_9185, X), // Respiratory Motion Compensation Technique Description
        (0x0018_9367, X), // X-Ray Source ID
        (0x0018_9371, X), // X-Ray Detector ID
        (0x0018_9373, X), // X-Ray Detector Label
        (0x0018_937B, X), // Multi-energy Acquisition Description
        (0x0018_9424, X), // Acquisition Protocol Description
        (0x0018_A003, X), // Contribution Description
        (0x0020_0010, Z), // Study ID
        (0x0020_4000, X), // Image Comments
        (0x0020_9158, X), // Frame Comments
        (0x0028_4000, X), // Image Presentation Comments
        (0x0032_0012, X), // Study ID Issuer
        (0x0032_1020, X), // Scheduled Study Location
        (0x0032_1021, X), // Scheduled Study Location AE Title
        (0x0032_1030, X), // Reason for Study
        (0x0032_1032, X), // Requesting Physician
        (0x0032_1033, X), // Requesting Service
        (0x0032_1060, X), // Requested Procedure Description
        (0x0032_1066, X), // Reason for Visit
        (0x0032_1067, X), // Reason for Visit Code Sequence
        (0x0032_1070, X), // Requested Contrast Agent
        (0x0032_4000, X), // Study Comments
        (0x0038_0004, X), // Referenced Patient Alias Sequence
        (0x0038_0010, X), // Admission ID
        (0x0038_0011, X), // Issuer of Admission ID
        (0x0038_0014, X), // Issuer of Admission ID Sequence
        (0x0038_0016, X), // Route of Admissions
        (0x0038_001A, X), // Scheduled Admission Date
        (0x0038_001B, X), // Scheduled Admission Time
        (0x0038_001C, X), // Scheduled Discharge Date
        (0x0038_001D, X), // Scheduled Discharge Time
        (0x0038_001E, X), // Scheduled Patient Institution Residence
        (0x0038_0020, X), // Admitting Date
        (0x0038_0021, X), // Admitting Time
        (0x0038_0030, X), // Discharge Date
        (0x0038_0032, X), // Discharge Time
        (0x0038_0040, X), // Discharge Diagnosis Description
        (0x0038_0050, X), // Special Needs
        (0x0038_0060, X), // Service Episode ID
        (0x0038_0061, X), // Issuer of Service Episode ID
        (0x0038_0062, X), // Service Episode Description
        (0x0038_0064, X), // Issuer of Service Episode ID Sequence
        (0x0038_0300, X), // Current Patient Location
        (0x0038_0400, X), // Patient's Institution Residence
        (0x0038_0500, X), // Patient State
        (0x0038_4000, X), // Visit Comments
        (0x0040_0001, X), // Scheduled Station AE Title
        (0x0040_0002, X), // Scheduled Procedure Step Start Date
        (0x0040_0003, X), // Scheduled Procedure Step Start Time
        (0x0040_0004, X), // Scheduled Procedure Step End Date
        (0x0040_0005, X), // Scheduled Procedure Step End Time
        (0x0040_0006, X), // Scheduled Performing Physician's Name
        (0x0040_0007, X), // Scheduled Procedure Step Description
        (0x0040_0009, X), // Scheduled Procedure Step ID
        (0x0040_000B, X), // Scheduled Performing Physician Identification Sequence
        (0x0040_0010, X), // Scheduled Station Name
        (0x0040_0011, X), // Scheduled Procedure Step Location
        (0x0040_0012, X), // Pre-Medication
        (0x0040_0241, X), // Performed Station AE Title
        (0x0040_0242, X), // Performed Station Name
        (0x0040_0243, X), // Performed Location
        (0x0040_0253, X), // Performed Procedure Step ID
        (0x0040_0254, X), // Performed Procedure Step Description
        (0x0040_0275, X), // Request Attributes Sequence
        (0x0040_0280, X), // Comments on the Performed Procedure Step
        (0x0040_0310, X), // Comments on Radiation Dose
        (0x0040_050A, X), // Specimen Accession Number
        (0x0040_0512, X), // Container Identifier
        (0x0040_0551, X), // Specimen Identifier
        (0x0040_0555, X), // Acquisition Context Sequence
        (0x0040_0600, X), // Specimen Short Description
        (0x0040_0602, X), // Specimen Detailed Description
        (0x0040_0610, X), // Specimen Preparation Sequence
        (0x0040_06FA, X), // Slide Identifier
        (0x0040_1001, X), // Requested Procedure ID
        (0x0040_1002, X), // Reason for the Requested Procedure
        (0x0040_1004, X), // Patient Transport Arrangements
        (0x0040_1005, X), // Requested Procedure Location
        (0x0040_100A, X), // Reason for Requested Procedure Code Sequence
        (0x0040_1010, X), // Names of Intended Recipients of Results
        (0x0040_1011, X), // Intended Recipients of Results Identification Sequence
        (0x0040_1101, X), // Person Identification Code Sequence
        (0x0040_1102, X), // Person's Address
        (0x0040_1103, X), // Person's Telephone Numbers
        (0x0040_1400, X), // Requested Procedure Comments
        (0x0040_2001, X), // Reason for the Imaging Service Request
        (0x0040_2008, X), // Order Entered By
        (0x0040_2009, X), // Order Enterer's Location
        (0x0040_2010, X), // Order Callback Phone Number
        (0x0040_2016, Z), // Placer Order Number / Imaging Service Request
        (0x0040_2017, Z), // Filler Order Number / Imaging Service Request
        (0x0040_2400, X), // Imaging Service Request Comments
        (0x0040_3001, X), // Confidentiality Constraint on Patient Data Description
        (0x0040_4034, X), // Scheduled Human Performers Sequence
        (0x0040_4035, X), // Actual Human Performers Sequence
        (0x0040_4036, X), // Human Performer's Organization
        (0x0040_4037, X), // Human Performer's Name
        (0x0040_A027, X), // Verifying Organization
        (0x0040_A078, X), // Author Observer Sequence
        (0x0040_A07A, X), // Participant Sequence
        (0x0040_A07C, X), // Custodial Organization Sequence
        (0x0040_A088, X), // Verifying Observer Identification Code Sequence
        (0x0040_A160, X), // Text Value
        (0x0040_A730, X), // Content Sequence
        (0x0070_0001, X), // Graphic Annotation Sequence (its text)
        (0x0070_0086, X), // Content Creator's Identification Code Sequence
        (0x0088_0140, X), // Storage Media File-set UID
        (0x0088_0200, X), // Icon Image Sequence (a picture of the pixels)
        (0x0088_0904, X), // Topic Title
        (0x0088_0906, X), // Topic Subject
        (0x0088_0910, X), // Topic Author
        (0x0088_0912, X), // Topic Keywords
        (0x0400_0402, X), // Referenced Digital Signature Sequence
        (0x0400_0403, X), // Referenced SOP Instance MAC Sequence
        (0x0400_0404, X), // MAC
        (0x0400_0550, X), // Modified Attributes Sequence
        (0x0400_0561, X), // Original Attributes Sequence
        (0x2030_0020, X), // Text String
        (0x3006_0004, X), // Structure Set Name
        (0x3006_0006, X), // Structure Set Description
        (0x3006_0028, X), // ROI Description
        (0x3006_0088, X), // ROI Observation Description
        (0x3006_00A6, X), // ROI Interpreter
        (0x300A_0003, X), // RT Plan Name
        (0x300A_0004, X), // RT Plan Description
        (0x300A_000E, X), // Prescription Description
        (0x300A_0016, X), // Dose Reference Description
        (0x300E_0008, X), // Reviewer Name
        (0x4000_0010, X), // Arbitrary
        (0x4000_4000, X), // Text Comments
        (0xFFFA_FFFA, X), // Digital Signatures Sequence
        (0xFFFC_FFFC, X), // Data Set Trailing Padding
    ]
};

fn action(tag: u32) -> Option<Action> {
    PROFILE.binary_search_by_key(&tag, |(t, _)| *t).ok().map(|i| PROFILE[i].1)
}

/// Groups removed whole: curves (50xx) and overlays (60xx: their bitmaps, labels and comments)
/// and the retired results group (4008: interpretations, their authors and texts).
fn group_removed(group: u32) -> bool {
    (0x5000..=0x501E).contains(&group) || (0x6000..=0x601E).contains(&group) || group == 0x4008
}

/// Text VRs whose values are free text, and their longest value (characters; 0: no limit).
fn text_limit(vr: &[u8; 2]) -> Option<usize> {
    match vr {
        b"SH" => Some(16),
        b"LO" => Some(64),
        b"ST" => Some(1024),
        b"LT" => Some(10240),
        b"UC" | b"UT" => Some(0),
        _ => None,
    }
}

/// Sequences nested deeper than this are refused (the rewrite recurses on them).
const MAX_DEPTH: usize = 32;

/// Of the patient group, these are kept (changed below).
const PATIENT_KEPT: &[u32] =
    &[0x0010_0010, 0x0010_0020, 0x0010_0030, 0x0010_0040, 0x0010_1010, 0x0010_1020, 0x0010_1030];

fn text(v: &[u8]) -> String {
    String::from_utf8_lossy(v).trim_end_matches(['\0', ' ']).trim().to_string()
}

/// A value padded to an even length (UI with NUL, the others with a space).
fn padded(s: &str, vr: &[u8; 2]) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    if v.len() % 2 == 1 {
        v.push(if vr == b"UI" { 0 } else { b' ' });
    }
    v
}

/// A UID as the rewrite gives it: the standard's own UIDs (classes, syntaxes) are kept.
fn new_uid(old: &str, key: &[u8]) -> String {
    if old.is_empty() || old.starts_with("1.2.840.10008.") {
        return old.to_string();
    }
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("any key length");
    m.update(b"dicom-uid\0");
    m.update(old.as_bytes());
    let d = m.finalize().into_bytes();
    let n = u128::from_be_bytes(d[..16].try_into().unwrap());
    format!("2.25.{n}")
}

fn shift_da(s: &str, off: i64) -> Option<String> {
    let d = NaiveDate::parse_from_str(s.get(..8)?, "%Y%m%d").ok()?;
    Some((d.and_hms_opt(0, 0, 0)? - Duration::seconds(off)).format("%Y%m%d").to_string())
}

fn parse_tm(s: &str) -> Option<(NaiveTime, String)> {
    let (main, frac) = s.split_once('.').map_or((s, ""), |(a, b)| (a, b));
    let t = match main.len() {
        2 => NaiveTime::parse_from_str(&format!("{main}0000"), "%H%M%S"),
        4 => NaiveTime::parse_from_str(&format!("{main}00"), "%H%M%S"),
        6 => NaiveTime::parse_from_str(main, "%H%M%S"),
        _ => return None,
    }
    .ok()?;
    Some((t, if frac.is_empty() { String::new() } else { format!(".{frac}") }))
}

fn shift_dt(s: &str, off: i64) -> Option<String> {
    let (core, zone) = match s.find(['+', '-']) {
        Some(i) if i >= 8 => (&s[..i], &s[i..]),
        _ => (s, ""),
    };
    let (main, frac) = core.split_once('.').map_or((core, ""), |(a, b)| (a, b));
    let padded = format!("{main:0<14}");
    let t = NaiveDateTime::parse_from_str(padded.get(..14)?, "%Y%m%d%H%M%S").ok()?;
    let m = t - Duration::seconds(off);
    let full = m.format("%Y%m%d%H%M%S").to_string();
    Some(format!("{}{}{}{}", &full[..main.len().min(14)], if frac.is_empty() { "" } else { "." }, frac, zone))
}

/// Whether bytes look like a date (an element in implicit VR whose VR is not known here).
fn looks_like_da(v: &[u8]) -> bool {
    let t = text(v);
    t.len() == 8
        && t.bytes().all(|b| b.is_ascii_digit())
        && NaiveDate::parse_from_str(&t, "%Y%m%d").is_ok_and(|d| (1900..=2100).contains(&chrono::Datelike::year(&d)))
}

struct Ctx<'a> {
    r: &'a Rewrite<'a>,
    explicit: bool,
    burned_in: bool,
}

/// The elements of one dataset (or item), rewritten.
fn clean(els: Vec<Element>, cx: &mut Ctx, top: bool) -> Vec<Element> {
    let off = cx.r.offset_s;
    // pairs first: a date and its time move together
    let mut pair_values: std::collections::HashMap<u32, Vec<u8>> = std::collections::HashMap::new();
    if off != 0 {
        for (da, tm) in PAIRS {
            let d = els.iter().find(|e| e.tag == *da).and_then(|e| match &e.value {
                Value::Bytes(v) => Some(text(v)),
                _ => None,
            });
            let t = els.iter().find(|e| e.tag == *tm).and_then(|e| match &e.value {
                Value::Bytes(v) => Some(text(v)),
                _ => None,
            });
            let (Some(d), Some(t)) = (d, t) else { continue };
            let (Ok(date), Some((time, frac))) = (NaiveDate::parse_from_str(&d, "%Y%m%d"), parse_tm(&t)) else {
                continue;
            };
            let m = date.and_time(time) - Duration::seconds(off);
            pair_values.insert(*da, padded(&m.format("%Y%m%d").to_string(), b"DA"));
            let digits = t.split('.').next().unwrap_or("").len().min(6);
            let tm_s = m.format("%H%M%S").to_string();
            pair_values.insert(*tm, padded(&format!("{}{frac}", &tm_s[..digits.max(2)]), b"TM"));
        }
    }
    let mut out = vec![];
    for mut e in els {
        let tag = e.tag;
        let group = tag >> 16;
        if group % 2 == 1 || group_removed(group) || action(tag) == Some(Action::X) {
            continue; // private, or identifying
        }
        if group == 0x0010 && !PATIENT_KEPT.contains(&tag) {
            continue;
        }
        // the clinical trial module (subject and site IDs, sponsor, protocol): replaced below
        if group == 0x0012 {
            continue;
        }
        if let Value::Items(items) = e.value {
            e.value = Value::Items(items.into_iter().map(|it| clean(it, cx, false)).collect());
            out.push(e);
            continue;
        }
        let Value::Bytes(v) = &e.value else { continue };
        let vr = e.vr;
        let new: Option<Vec<u8>> = if action(tag) == Some(Action::Z) {
            Some(vec![])
        } else if tag == 0x0010_0010 {
            Some(padded(crate::gate::PATIENT_NAME, b"PN"))
        } else if tag == 0x0010_0020 {
            Some(padded(cx.r.pseudonym, b"LO"))
        } else if &vr == b"PN" {
            Some(if v.is_empty() { vec![] } else { padded(crate::gate::STAFF_NAME, b"PN") })
        } else if tag == 0x0010_1010 && cx.r.cap_ages {
            let t = text(v);
            let years = t.strip_suffix('Y').and_then(|n| n.parse::<u32>().ok());
            years.filter(|y| *y > 89).map(|_| padded("090Y", b"AS"))
        } else if tag == 0x0028_0301 {
            if text(v).eq_ignore_ascii_case("YES") {
                cx.burned_in = true;
            }
            None
        } else if &vr == b"UI" {
            let t = text(v);
            // several UIDs may be listed (VM > 1)
            let mapped: Vec<String> = t.split('\\').map(|u| new_uid(u.trim(), cx.r.uid_key)).collect();
            let joined = mapped.join("\\");
            (joined != t).then(|| padded(&joined, b"UI"))
        } else if off != 0 && pair_values.contains_key(&tag) {
            pair_values.get(&tag).cloned()
        } else if off != 0 && &vr == b"DA" {
            let t = text(v);
            let parts: Vec<String> = t.split('\\').map(|d| shift_da(d, off).unwrap_or_default()).collect();
            Some(padded(&parts.join("\\"), b"DA"))
        } else if off != 0 && &vr == b"DT" {
            let t = text(v);
            let parts: Vec<String> = t.split('\\').map(|d| shift_dt(d, off).unwrap_or_default()).collect();
            Some(padded(&parts.join("\\"), b"DT"))
        } else if off != 0 && &vr == b"TM" {
            let t = text(v);
            parse_tm(&t).map(|(time, frac)| {
                let m = time - Duration::seconds(off.rem_euclid(86400));
                let digits = t.split('.').next().unwrap_or("").len().clamp(2, 6);
                padded(&format!("{}{frac}", &m.format("%H%M%S").to_string()[..digits]), b"TM")
            })
        } else if off != 0 && !cx.explicit && &vr == b"UN" && looks_like_da(v) {
            shift_da(&text(v), off).map(|d| padded(&d, b"DA"))
        } else if let Some(limit) = text_limit(&vr) {
            free_text(v, &vr, limit, cx.r.text)
        } else if !cx.explicit && &vr == b"UN" && is_text(v) {
            // an element whose VR is not known here (implicit VR): text is scrubbed as text
            free_text(v, &vr, 0, cx.r.text)
        } else {
            None
        };
        if let Some(n) = new {
            e.value = Value::Bytes(n);
        }
        out.push(e);
    }
    if top {
        // LO: values of 64 characters at most
        let method = "Medorama Privacy Mode\\PS3.15 Table E.1-1 subset\\free text scrubbed\\dates moved back";
        let added: [(u32, [u8; 2], &str); 3] = [
            (0x0012_0062, *b"CS", "YES"),
            (0x0012_0063, *b"LO", method),
            (0x0028_0303, *b"CS", if off != 0 { "MODIFIED" } else { "UNMODIFIED" }),
        ];
        for (tag, vr, val) in added {
            out.retain(|e| e.tag != tag);
            out.push(Element { tag, vr, value: Value::Bytes(padded(val, &vr)) });
        }
        out.sort_by_key(|e| e.tag);
    }
    out
}

/// Whether a value of unknown VR is text (printable, not binary numbers).
fn is_text(v: &[u8]) -> bool {
    let t = v.strip_suffix(&[0]).unwrap_or(v);
    !t.is_empty()
        && std::str::from_utf8(t)
            .is_ok_and(|s| s.chars().all(|c| !c.is_control() || c == '\n' || c == '\r' || c == '\t'))
}

/// A free-text value through [`Rewrite::text`] (each value of a multi-valued one), cut to its
/// VR's length (`limit` characters, 0: none); `None` when nothing changes.
fn free_text(v: &[u8], vr: &[u8; 2], limit: usize, scrub: &dyn Fn(&str) -> String) -> Option<Vec<u8>> {
    let t = String::from_utf8_lossy(v);
    let t = t.trim_end_matches(['\0', ' ']);
    // ST, LT and UT hold one value, in which a backslash is text
    let multi = matches!(vr, b"SH" | b"LO" | b"UC" | b"UN");
    let parts: Vec<String> = if multi { t.split('\\').map(scrub).collect() } else { vec![scrub(t)] };
    let parts: Vec<String> =
        parts.into_iter().map(|x| if limit > 0 { x.chars().take(limit).collect() } else { x }).collect();
    let joined = parts.join("\\");
    (joined != t).then(|| padded(&joined, vr))
}

fn write_elements(els: &[Element], enc: Enc, out: &mut Vec<u8>) {
    for e in els {
        out.extend_from_slice(&((e.tag >> 16) as u16).to_le_bytes());
        out.extend_from_slice(&((e.tag & 0xFFFF) as u16).to_le_bytes());
        match &e.value {
            Value::Items(items) => {
                if enc.explicit {
                    out.extend_from_slice(b"SQ\0\0");
                }
                out.extend_from_slice(&UNDEFINED.to_le_bytes());
                for it in items {
                    out.extend_from_slice(&[0xFE, 0xFF, 0x00, 0xE0]);
                    out.extend_from_slice(&UNDEFINED.to_le_bytes());
                    write_elements(it, enc, out);
                    out.extend_from_slice(&[0xFE, 0xFF, 0x0D, 0xE0, 0, 0, 0, 0]);
                }
                out.extend_from_slice(&[0xFE, 0xFF, 0xDD, 0xE0, 0, 0, 0, 0]);
            }
            Value::Bytes(v) => {
                if enc.explicit {
                    out.extend_from_slice(&e.vr);
                    if LONG_VR.contains(&&e.vr) {
                        out.extend_from_slice(&[0, 0]);
                        out.extend_from_slice(&(v.len() as u32).to_le_bytes());
                    } else {
                        out.extend_from_slice(&(v.len() as u16).to_le_bytes());
                    }
                } else {
                    out.extend_from_slice(&(v.len() as u32).to_le_bytes());
                }
                out.extend_from_slice(v);
            }
        }
    }
}

fn rewrite(b: &[u8], r: &Rewrite, complete: bool) -> Result<Rewritten, Error> {
    let part10 = b.len() >= 132 && &b[128..132] == b"DICM";
    let mut head = vec![];
    let mut pos = 0;
    let mut syntax = IMPLICIT.to_string();
    let mut cx = Ctx { r, explicit: false, burned_in: false };
    if part10 {
        head.extend_from_slice(&[0u8; 128]);
        head.extend_from_slice(b"DICM");
        pos = 132;
        // the file meta information: explicit VR little endian, group 0002
        let mut meta = vec![];
        loop {
            let Some(tag) = tag_at(b, pos) else { return Err(Error::NeedMore) };
            if tag >> 16 != 0x0002 {
                break;
            }
            // one element at a time, bounded by its own length
            let vr: [u8; 2] = b.get(pos + 4..pos + 6).ok_or(Error::NeedMore)?.try_into().unwrap();
            let (len, hdr) = if LONG_VR.contains(&&vr) {
                (le32(b, pos + 8).ok_or(Error::NeedMore)?, 12)
            } else {
                (le16(b, pos + 6).ok_or(Error::NeedMore)? as u32, 8)
            };
            if len == UNDEFINED {
                return bad("an undefined length in the file meta information");
            }
            let end = pos + hdr + len as usize;
            let (one, after, _) = read_elements(b, pos, Some(end), Enc { explicit: true }, false, complete, 0)?;
            pos = after;
            meta.extend(one);
        }
        for e in meta.iter() {
            if e.tag == 0x0002_0010 {
                if let Value::Bytes(v) = &e.value {
                    syntax = text(v);
                }
            }
        }
        if syntax == BIG_ENDIAN || syntax == DEFLATED {
            return bad(format!("DICOM in transfer syntax {syntax} is not sent in Privacy Mode"));
        }
        cx.explicit = true;
        let mut meta: Vec<Element> = meta.into_iter().filter(|e| e.tag != 0x0002_0000).collect();
        meta = clean_meta(meta, r);
        let mut body = vec![];
        write_elements(&meta, Enc { explicit: true }, &mut body);
        head.extend_from_slice(&[0x02, 0x00, 0x00, 0x00, b'U', b'L', 4, 0]);
        head.extend_from_slice(&(body.len() as u32).to_le_bytes());
        head.extend_from_slice(&body);
    }
    let enc = Enc { explicit: part10 && syntax != IMPLICIT };
    cx.explicit = enc.explicit;
    let (els, after, pixels) = read_elements(b, pos, None, enc, true, complete, 0)?;
    if !pixels && after < b.len() {
        return bad("trailing bytes after the dataset");
    }
    let els = clean(els.into_iter().filter(|e| e.tag & 0xFFFF != 0x0000).collect(), &mut cx, true);
    write_elements(&els, enc, &mut head);
    Ok(Rewritten { head, rest_from: if pixels { after as u64 } else { b.len() as u64 }, burned_in: cx.burned_in })
}

/// The file meta information: the instance UID as in the dataset, private information removed.
fn clean_meta(meta: Vec<Element>, r: &Rewrite) -> Vec<Element> {
    meta.into_iter()
        .filter(|e| action(e.tag) != Some(Action::X))
        .map(|mut e| {
            if e.tag == 0x0002_0003 {
                if let Value::Bytes(v) = &e.value {
                    e.value = Value::Bytes(padded(&new_uid(&text(v), r.uid_key), b"UI"));
                }
            }
            e
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An explicit-VR element.
    fn el(tag: u32, vr: &[u8; 2], v: &[u8]) -> Vec<u8> {
        let mut out = vec![];
        out.extend_from_slice(&((tag >> 16) as u16).to_le_bytes());
        out.extend_from_slice(&((tag & 0xFFFF) as u16).to_le_bytes());
        out.extend_from_slice(vr);
        let v = padded(std::str::from_utf8(v).unwrap(), vr);
        if LONG_VR.contains(&vr) {
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(&(v.len() as u32).to_le_bytes());
        } else {
            out.extend_from_slice(&(v.len() as u16).to_le_bytes());
        }
        out.extend_from_slice(&v);
        out
    }

    fn file(syntax: &str, dataset: &[u8]) -> Vec<u8> {
        let mut meta = el(0x0002_0003, b"UI", b"1.2.3.4.5.6");
        meta.extend(el(0x0002_0010, b"UI", syntax.as_bytes()));
        meta.extend(el(0x0002_0016, b"AE", b"SITE_PACS"));
        let mut out = vec![0u8; 128];
        out.extend_from_slice(b"DICM");
        out.extend_from_slice(&[0x02, 0, 0, 0, b'U', b'L', 4, 0]);
        out.extend_from_slice(&(meta.len() as u32).to_le_bytes());
        out.extend(meta);
        out.extend_from_slice(dataset);
        out
    }

    #[test]
    fn identifiers_go_dates_move_pixels_stay() {
        let mut ds = vec![];
        ds.extend(el(0x0008_0018, b"UI", b"1.2.3.4.5.6"));
        ds.extend(el(0x0008_0020, b"DA", b"20240115"));
        ds.extend(el(0x0008_0030, b"TM", b"083000"));
        ds.extend(el(0x0008_0050, b"SH", b"ACC123456"));
        ds.extend(el(0x0008_0080, b"LO", b"General Hospital"));
        ds.extend(el(0x0008_0090, b"PN", b"Smith^Referring"));
        // a sequence holding a person's name
        let mut seq = vec![0x08, 0x00, 0x15, 0x11, b'S', b'Q', 0, 0];
        let item_body = el(0x0040_A123, b"PN", b"Osei^Observer");
        let mut item = vec![0xFE, 0xFF, 0x00, 0xE0];
        item.extend_from_slice(&(item_body.len() as u32).to_le_bytes());
        item.extend(item_body);
        seq.extend_from_slice(&(item.len() as u32).to_le_bytes());
        seq.extend(item);
        ds.extend(seq);
        ds.extend(el(0x0009_0010, b"LO", b"PRIVATE CREATOR"));
        ds.extend(el(0x0010_0010, b"PN", b"Doe^Jane"));
        ds.extend(el(0x0010_0020, b"LO", b"MRN7654321"));
        ds.extend(el(0x0010_0030, b"DA", b"19300304"));
        ds.extend(el(0x0010_0040, b"CS", b"F"));
        ds.extend(el(0x0010_1000, b"LO", b"OTHER-ID-1"));
        ds.extend(el(0x0010_1010, b"AS", b"094Y"));
        ds.extend(el(0x0020_000D, b"UI", b"1.2.3.4.7"));
        ds.extend(el(0x0028_0301, b"CS", b"YES"));
        let pixels = el(PIXEL_DATA, b"OW", &[b'P'; 16]);
        ds.extend(&pixels);
        let bytes = file("1.2.840.10008.1.2.1", &ds);
        let dir = std::env::temp_dir().join(format!("medorama-dicom-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("one.dcm");
        std::fs::write(&path, &bytes).unwrap();
        let r = Rewrite {
            pseudonym: "MO-7F3K-2QXM",
            offset_s: 36570 * 86400,
            cap_ages: true,
            uid_key: b"secret",
            text: &|s| s.to_string(),
        };
        let out = deidentify(&path, &r).unwrap();
        let head = String::from_utf8_lossy(&out.head).to_string();
        for gone in [
            "Doe",
            "Jane",
            "MRN7654321",
            "19300304",
            "Smith",
            "Osei",
            "General Hospital",
            "ACC123456",
            "OTHER-ID-1",
            "PRIVATE",
            "SITE_PACS",
            "1.2.3.4.5.6",
            "1.2.3.4.7",
            "20240115",
        ] {
            assert!(!head.contains(gone), "{gone} is still there");
        }
        for kept in ["[Patient Name]", "MO-7F3K-2QXM", "[Staff Name]", "090Y", "2.25.", "YES", "MODIFIED"] {
            assert!(head.contains(kept), "{kept} is missing");
        }
        // 2024-01-15 08:30 moved back 36570 days: 1923-12-01 08:30
        assert!(head.contains("19231201") && head.contains("083000"), "{head}");
        assert!(out.burned_in);
        // the pixel data is the original's, from where it started
        let rest = &bytes[out.rest_from as usize..];
        assert_eq!(rest, pixels.as_slice());
        // the same UID maps the same way (a series holds together), differently with another key
        assert_eq!(new_uid("1.2.3.4.7", b"secret"), new_uid("1.2.3.4.7", b"secret"));
        assert_ne!(new_uid("1.2.3.4.7", b"secret"), new_uid("1.2.3.4.7", b"other"));
        assert_eq!(
            new_uid("1.2.840.10008.5.1.4.1.1.2", b"secret"),
            "1.2.840.10008.5.1.4.1.1.2",
            "the standard's UIDs stay"
        );
        // the rewritten file reads again (a second pass finds nothing left to change but UIDs)
        let mut again = out.head.clone();
        again.extend_from_slice(rest);
        std::fs::write(&path, &again).unwrap();
        let twice = deidentify(&path, &r).unwrap();
        assert!(!String::from_utf8_lossy(&twice.head).contains("Doe"));
        // refused: deflated and big-endian files
        std::fs::write(&path, file(DEFLATED, &[])).unwrap();
        assert!(deidentify(&path, &r).is_err());
        // truncated anywhere: an error, never a panic
        for cut in (0..bytes.len()).step_by(7) {
            std::fs::write(&path, &bytes[..cut]).unwrap();
            let _ = deidentify(&path, &r);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    fn rewrite_of(syntax: &str, ds: &[u8]) -> anyhow::Result<Rewritten> {
        let scrub = |s: &str| s.replace("Zyxwortha", crate::gate::PATIENT_NAME);
        let r = Rewrite { pseudonym: "MO-AAAA-AAAA", offset_s: 86400, cap_ages: true, uid_key: b"k", text: &scrub };
        deidentify_bytes(&file(syntax, ds), &r)
    }

    fn has_tag(head: &[u8], tag: u32) -> bool {
        let t = [((tag >> 16) as u16).to_le_bytes(), ((tag & 0xFFFF) as u16).to_le_bytes()].concat();
        head.windows(4).any(|w| w == t.as_slice())
    }

    #[test]
    fn the_profile_table_is_sorted() {
        assert!(PROFILE.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn free_text_elements_go_or_are_scrubbed() {
        let removed: [(u32, &[u8; 2]); 11] = [
            (0x0008_1030, b"LO"), // Study Description
            (0x0008_103E, b"LO"), // Series Description
            (0x0008_2111, b"ST"), // Derivation Description
            (0x0018_4000, b"LT"), // Acquisition Comments
            (0x0032_1060, b"LO"), // Requested Procedure Description
            (0x0040_0254, b"LO"), // Performed Procedure Step Description
            (0x0040_0280, b"ST"), // Comments on the Performed Procedure Step
            (0x0008_4000, b"LT"), // Identifying Comments
            (0x0040_A160, b"UT"), // Text Value
            (0x0008_1080, b"LO"), // Admitting Diagnoses Description
            (0x6000_4000, b"LT"), // Overlay Comments
        ];
        let mut ds = vec![];
        for (tag, vr) in removed {
            ds.extend(el(tag, vr, b"for Zyxwortha"));
        }
        // a free-text element the table does not name (Body Part Examined is CS; Series of a
        // Manufacturer's Model Name is LO): kept, scrubbed
        ds.extend(el(0x0008_1090, b"LO", b"Model of Zyxwortha"));
        ds.extend(el(0x0018_0015, b"CS", b"CHEST"));
        let out = rewrite_of("1.2.840.10008.1.2.1", &ds).unwrap();
        for (tag, _) in removed {
            assert!(!has_tag(&out.head, tag), "{tag:08X} kept");
        }
        let head = String::from_utf8_lossy(&out.head);
        assert!(!head.contains("Zyxwortha"), "{head}");
        assert!(head.contains("Model of [Patient Name]") && head.contains("CHEST"), "{head}");
    }

    #[test]
    fn implicit_vr_names_and_text() {
        // implicit VR little endian: tag, 32-bit length, value
        let el = |tag: u32, v: &[u8]| {
            let v = padded(std::str::from_utf8(v).unwrap(), b"LO");
            let mut out = vec![];
            out.extend_from_slice(&((tag >> 16) as u16).to_le_bytes());
            out.extend_from_slice(&((tag & 0xFFFF) as u16).to_le_bytes());
            out.extend_from_slice(&(v.len() as u32).to_le_bytes());
            out.extend(v);
            out
        };
        let mut ds = el(0x0008_1070, b"Osei^Kwame"); // Operators' Name (PN)
        ds.extend(el(0x0008_1090, b"Zyxwortha special")); // Manufacturer's Model Name (LO)
        ds.extend(el(0x0028_0010, &[0x00, 0x02])); // Rows (US): binary, kept as it is
        ds.extend(el(0x300E_0008, b"Osei^K")); // Reviewer Name: removed
        let out = rewrite_of(IMPLICIT, &ds).unwrap();
        let head = String::from_utf8_lossy(&out.head);
        assert!(!head.contains("Osei") && !head.contains("Zyxwortha"), "{head}");
        assert!(head.contains("[Staff Name]") && head.contains("[Patient Name] special"), "{head}");
        assert!(out.head.windows(8).any(|w| w == [0x28, 0, 0x10, 0, 2, 0, 0, 0]), "Rows kept");
    }

    #[test]
    fn deep_sequences_are_refused() {
        let nested = |depth: usize| {
            let mut ds = vec![];
            for _ in 0..depth {
                ds.extend_from_slice(&[0x40, 0x00, 0x30, 0xA7, b'S', b'Q', 0, 0, 0xFF, 0xFF, 0xFF, 0xFF]);
                ds.extend_from_slice(&[0xFE, 0xFF, 0x00, 0xE0, 0xFF, 0xFF, 0xFF, 0xFF]);
            }
            for _ in 0..depth {
                ds.extend_from_slice(&[0xFE, 0xFF, 0x0D, 0xE0, 0, 0, 0, 0]);
                ds.extend_from_slice(&[0xFE, 0xFF, 0xDD, 0xE0, 0, 0, 0, 0]);
            }
            ds
        };
        assert!(rewrite_of("1.2.840.10008.1.2.1", &nested(8)).is_ok());
        // as on a blocking thread of the server (2 MiB of stack): an error, not an abort
        let deep = nested(5000);
        let r = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || rewrite_of("1.2.840.10008.1.2.1", &deep).map(|_| ()).map_err(|e| e.to_string()))
            .unwrap()
            .join()
            .unwrap();
        assert!(r.unwrap_err().contains("nested"));
    }

    #[test]
    fn dates_and_times() {
        assert_eq!(shift_da("20240115", 86400).unwrap(), "20240114");
        assert_eq!(shift_dt("20240115083000.5+0100", 3600).unwrap(), "20240115073000.5+0100");
        assert_eq!(shift_dt("2024011508", 3600).unwrap(), "2024011507");
        assert!(looks_like_da(b"20240115") && !looks_like_da(b"12345678") && !looks_like_da(b"2024011"));
    }
}
