# How Medorama protects patient data

This page explains how Medorama keeps patients' identities from people who are not allowed to
see them. It is written for everyone who works with Medorama: users who want to know what
Privacy Mode shows, data managers who prepare data, and privacy officers and IT security
reviewers who must judge the design. The explanations are in plain language; each section ends
with references to the code and tests, for reviewers who want to check them. The complete
security review, with the threat model and the deployment checklist, is
[security.md](security.md).

## Contents

- [The principle](#the-principle)
- [Terms used on this page](#terms-used-on-this-page)
- [Where identifiers are kept](#where-identifiers-are-kept)
- [Privacy Mode and the PHI permission](#privacy-mode-and-the-phi-permission)
- [What is withheld and what replaces it](#what-is-withheld-and-what-replaces-it)
- [De-identifying files at the source](#de-identifying-files-at-the-source)
- [The VitalStream viewer](#the-vitalstream-viewer)
- [Exports and plug-in actions](#exports-and-plug-in-actions)
- [Connected databases](#connected-databases)
- [Failing closed](#failing-closed)
- [How the design is verified](#how-the-design-is-verified)
- [What Privacy Mode cannot do](#what-privacy-mode-cannot-do)
- [The repository holds no patient data](#the-repository-holds-no-patient-data)

## The principle

Medorama brings together data that identify patients: names, medical record numbers, dates of
birth, account numbers, and the dates of every event. Its design rests on three rules.

1. **Identifiers stay in the databases.** Medorama imports nothing: it reads each connected
   database in place. The identifiers it needs to join databases are kept only in that
   database's own encrypted file in Medorama's workspace, and Medorama's own database (the
   control store) holds none. The viewers, the waveform indexes, the results of analyses and the
   plug-in programs never receive them.
2. **What leaves the server is decided at the moment of sending.** Privacy Mode is a property
   of a user's session, not of the data. For each answer, the server checks who is asking and
   whether that session is in Privacy Mode, and then sends either the identified data or a
   de-identified copy made on the spot.
3. **Nothing identifying reaches the browser of a user who may not see it.** The page in the
   browser does no masking of its own: it never receives what it must not show. A user who
   opens the browser's developer tools, or calls the API directly, gets the same de-identified
   answers that the page displays.

The part of the server that makes the de-identified copy is called the **privacy gate**.

![The privacy gate: what leaves in Privacy Mode](images/privacy-gate.svg)

The two screenshots below show the same synthetic patient. On the left, an administrator with
the PHI permission sees the identified record; on the right, the same page in Privacy Mode
shows placeholders, a pseudonym and dates moved back by about a century.

| Identified | Privacy Mode |
|---|---|
| ![The patient page, identified](images/screens/patient.png) | ![The same patient in Privacy Mode](images/screens/privacy.png) |

## Terms used on this page

| Term | Meaning |
|---|---|
| PHI | Protected health information: anything that can identify a patient, together with their health data. |
| Connected database | A database that Medorama reads in place, read-only: an OMOP instance, any SQL database through views, or a SQLite file made by db-builder. Medorama imports nothing. |
| Control store | Medorama's own database: users, grants, the audit log, the list of connected databases, cohort definitions, patient numbers and pseudonyms, wrapped keys (SQLite or PostgreSQL). It holds no identifier. |
| Workspace | The folder where Medorama keeps what it generates from each database (its persons as linked, its variables, its recordings' state) and each cohort, each encrypted with its own key. |
| Identifier | A value that names a person directly: a name, a medical record number (MRN), a date of birth, a database's patient ID. |
| Grant | A right to use one database (`search`, `view`, `phi`, `manage`) or one cohort, given to a user. |
| PHI permission | A right given to a user account by an administrator. Without it, a user can never see identifiers. With it, a user sees the identifiers of the databases on which they hold the `phi` grant. |
| Privacy Mode | The state of a session in which every answer passes the privacy gate. |
| Privacy gate | The server code that de-identifies answers and files for sessions in Privacy Mode, and refuses what it cannot de-identify. |
| Pseudonym | A random code that stands for a patient (`MO-XXXX-XXXX`) or a visit (`MO-E-XXXX-XXXX`) in Privacy Mode. |
| Placeholder | Fixed text that stands for a withheld value, such as `[Patient Name]` or `[MRN]`. |
| Date offset | The amount by which Privacy Mode moves a patient's dates back: Medorama's offset plus the patient's own number of days. |

## Where identifiers are kept

- **In the connected databases**, which Medorama only reads. Their encryption, backups and
  accounts are their owners'.
- **In each database's `persons.sqlite`**, in its folder of Medorama's workspace: the identifiers
  of the database's persons as Medorama linked them (patient IDs, MRNs, names, dates of birth,
  sex), which the privacy gate needs to recognize them in answers. The file is encrypted with
  SQLCipher (AES-256) with that database's own data key; the key is kept only wrapped by a master
  key that is not in the workspace. Without the master key, a copy of the workspace cannot be read.
- **Not in the control store.** To join databases, Medorama keeps only a keyed hash
  (HMAC-SHA-256) of each (issuer, identifier) pair, and the patient number it leads to. Two
  databases that name a patient by the same issuer and identifier are one patient; Medorama never
  guesses or merges. The secret that keys the hash is kept wrapped by the master key, like the
  data keys: a copy of the control store alone cannot test identifiers against it (MRNs are few
  enough to be tried one by one by whoever holds the secret).
- **Not in the indexes or the plug-ins' output**, which hold signals, times and pseudonyms. The
  indexes of a database's recordings are in its workspace folder (`index.sqlite`), encrypted with
  the same data key, and destroyed with it. The plug-ins' output folders are not encrypted in this
  version: keep the workspace on an encrypted disk.

**Grants.** A user sees a database's patients only when granted it, and its identifiers only
with the `phi` grant on it and the PHI permission, outside Privacy Mode. The rule holds per
database: a user with the PHI permission, Privacy Mode off, who holds `phi` on one database and
only `view` on another, gets the second database's data about the same patient as Privacy Mode
shows them (dates moved back, free text through the gate, authors as `[Staff Name]`, files by
tokens and de-identified), and the first's identified. An administrator is not
granted a database by being an administrator, and an administrator who grants themselves one is
written to the audit log.

**Removing a database** destroys its data key first, then its workspace folder, then every
cohort row it contributed to, its patient links, and the patients and pseudonyms that no other
database holds. From then on nothing Medorama generated from it can be read, even from a copy of
the disk. A backup of the control store and the workspace made before the removal can still
bring it back until the backup expires; the audit log keeps its entries, which name patients only
by numbers that lead nowhere once the links are gone
([administration.md, Removing a database](administration.md#removing-a-database)).

**For reviewers:** `workspace.rs` (keys, SQLCipher files, shredding), `persons.rs` (the patient
index), `grants.rs`, `databases.rs` (`remove`); tested in `medorama/tests/databases.rs` and the
unit tests of `workspace.rs`. [security.md, Data at rest](security.md#8-data-at-rest) gives the
detail.

## Privacy Mode and the PHI permission

### Who is in Privacy Mode

Every session is in Privacy Mode unless its user has the PHI permission and has switched
Privacy Mode off. The defaults are as follows.

| User | Privacy Mode |
|---|---|
| Without the PHI permission (any role) | Always on. It cannot be switched off. |
| With the PHI permission, not an administrator | On when the user signs in. The user may switch it off. |
| Administrator with the PHI permission | Off when the user signs in. The administrator may switch it on. |

The role of a user (viewer, analyst or administrator) decides what the user may *do*; grants
decide which databases the user may use; the PHI permission decides whether the user may *see*
identifiers, and then only those of the databases granted `phi`. An administrator grants the PHI
permission when creating an account, or later in Admin › Users
([administration.md](administration.md)). Most users should not have it: they work with
pseudonyms and shifted dates, which is enough for most research.

### Switching Privacy Mode

The **Privacy Mode** switch in the page's header changes the mode for that session. The server
keeps the choice with the session, never in the browser, and writes an entry to the audit log
(`privacy_on` or `privacy_off`): leaving Privacy Mode is always audited. The page then reloads,
so that nothing identified stays on the screen after Privacy Mode is switched on. A user
without the PHI permission who asks to leave Privacy Mode is refused (HTTP 403). Every sign-in
starts in the user's default mode, so whoever uses the browser next starts there too.

Signing out, or a session that ends while the page is open, reloads the page: the next person
to sign in at that browser sees nothing of what the previous user saw.

A session in Privacy Mode is treated exactly like a user without the PHI permission: the server
replaces the session's user with a copy that has no PHI permission
(`users.rs`, `User::in_privacy_mode`), so every check that depends on the permission fails
closed for that session.

**For reviewers:** the decision is `server.rs` `in_privacy` and `session_user`; the switch is
`server.rs` `set_privacy`. Tested in `medorama/tests/privacy.rs` (a user without the
permission is refused; an administrator who switches Privacy Mode on receives no identifier).

## What is withheld and what replaces it

### Identifiers and dates

The table lists what Privacy Mode withholds by default and what the user sees instead. The
defaults follow the identifiers of the HIPAA Safe Harbor list that the databases' canonical
columns hold, with one exception, the text of notes, described below.

| Data | In Privacy Mode |
|---|---|
| Patients' names | `[Patient Name]` |
| Staff names (authors of notes, ECG overreaders, physicians) | `[Staff Name]` |
| Medical record numbers | `[MRN]` |
| Dates of birth | `[Date of Birth]`. The age is computed on the server and shown instead. |
| The databases' patient IDs | The patient's pseudonym, `MO-XXXX-XXXX` (a patient ID may be an MRN). |
| Visits | The visit's pseudonym, `MO-E-XXXX-XXXX`; the database's own key for a visit never leaves the server. |
| Every date and time | Moved back by the patient's date offset ([Dates and ages](#dates-and-ages)). |
| Ages above 89 | `90+` |
| Recordings' paths and file names | Tokens (`mo-rec:<n>`, `mo-f0003.dcm`), and `[Recording]` or `[File Name]` where a name is shown. |
| The text of notes and reports, and every other free text (any entry's text, a long written value) | Shown by default, with the patient's own names and date of birth, telephone numbers, e-mail and web addresses, social security numbers, street addresses and labelled MRNs replaced, and marked with a warning. Withheld (`[Withheld]`) when the administrator chooses so ([Text and notes](#text-and-notes)). |

Placeholders are fixed text. They never reveal the length or the form of the value they
replace.

### Pseudonyms

Each patient receives a pseudonym such as `MO-D5QM-Y1X5` when first linked, and each visit one
such as `MO-E-QMC6-E2VA` when first shown. Users find patients by these pseudonyms in Privacy Mode, and
exports name patients by them.

- **Random and stored, never derived.** The seven characters come from the operating system's
  secure random generator; an eighth is a check character. No calculation on an MRN or a name
  produces the pseudonym: the only link between a pseudonym and a person is the patient number
  that both the control store and the database's encrypted persons file hold. This follows HIPAA 45 CFR §164.514(c), which asks that a re-identification code
  not be "derived from or related to information about the individual", and ISO/TS 25237.
- **Why not a hash of the MRN.** MRNs are short, so every possible MRN can be hashed in
  minutes, which would reverse an unkeyed hash. A keyed hash would depend on one secret
  forever and would still be derived from the identifier.
- **Stable.** A patient keeps the same pseudonym through refreshes and restarts, as long as a
  connected database links them. A patient that no database links any more (its databases
  removed) is deleted with the pseudonym.
- **Checked.** The check character (Luhn mod 32, in Crockford's base 32, which has no I, L, O
  or U) catches any single mistyped character, so a pseudonym typed into a search is either
  right or refused.
- **The same everywhere.** Pages, exports, cohort definitions, DICOM headers and plug-ins'
  job descriptions use the same pseudonym.

Medorama set up again from scratch, or a database removed and connected again, draws new
pseudonyms. Keep backups of the control store to keep pseudonyms stable.

### Dates and ages

Research depends on the time between events, so Privacy Mode moves dates rather than removing
them.

- **Medorama's offset.** Every date and time moves back by an offset that the administrator
  sets in years, months, days and hours (Admin › Privacy Mode). A new installation draws it at
  random (80 to 120 years and 0 to 364 days), from the system's secure generator, so that the
  offset is known only to its administrators. An installation made before this keeps the
  former documented default (36,570 days, about 100 years), and `medorama serve` warns at start
  until an administrator sets another (doing so moves every date as shown).
- **Each patient's own days.** On top of that, each patient's dates move back by a whole number
  of days between 0 and a maximum (`spread_days`: 365 by default, at least 30 and at most
  3,650; a smaller value saved before that minimum is kept, since raising it moves every
  patient's dates, and `medorama serve` warns about it at start). This number
  is drawn at random once, when the patient's pseudonym is drawn, and stored with it. Every user
  and every session therefore sees the same dates for the same patient.
- **Intervals are kept.** The time between any two events of one patient is exact. Between two
  patients it changes by the difference of their own days.
- **Everywhere.** Dates move in every form Medorama sends: numbers (Unix times),
  text (`2024-01-02` with or without a time, `2024/01/02`, `01/02/2024`, `2.1.2024`,
  `02-JAN-2024`, `Jan 2, 2024`, `2 January 2024`, `20240102`), BedMaster file names' Unix times
  (a name whose moved time falls before 1970 is written with a minus sign:
  `ICU7_12--1455465600`), DICOM dates and times, GE MUSE XML dates, WFDB base dates, EDF start
  dates, and the keys of entries that carry a time. A date written `01/02/2024` is read month
  first, unless Medorama writes dates day first (Admin › Dates and times) or the month would
  be over 12; `2.1.2024` is read day first. Either way the year moves back by about a century,
  so a misread date shows no real date. A Unix time written on its own is a number, and is
  kept.
- **Searches and criteria in Privacy Mode.** A search, a filter or a sort uses only what the
  session is shown (pseudonyms, file tokens, each patient's dates as shown), so it cannot test a
  real value. A date a user types, in a cohort criterion or a date filter, is a date as shown:
  the server compares it with each patient's dates as that session sees them, so the criterion
  means exactly what the page says. The page never holds the date offset: only administrators
  may read it (`GET deid`).
- **Ages.** Ages are computed on the server from the date of birth, which never leaves it.
  Ages above 89 are shown as `90+`, and an age criterion tests them as 90: only "90 or more"
  finds them, and the reason reads `age 90+`.
- **More forms of dates in text.** Besides those above: two-digit years (`03/11/24`,
  `11.03.24`, `11-MAR-24`, read in this century unless that is ahead, and written in full once
  moved), HL7 and DICOM date-times (`202403110830`, `20240311083000`), a month and its year
  (`March 2024`, `03/2024`), ordinals (`2nd March 2024`, `March 2nd, 2024`), abbreviations
  with a dot and `Sept` (`Mar. 2, 2024`), and `2024.03.11`.

Date shifting is pseudonymization, not anonymization. Anyone who learns the real date of one
of a patient's events can re-date all of that patient's events, and every other patient's to
within `spread_days`. Keep the offset confidential.

### Text and notes

Free text, such as clinical notes and reports, can name the patient, relatives or staff in
ways that no rule can recognize reliably. By default, Privacy Mode shows the text of notes,
because it is often essential for research, with the following protections:

- the patient's own names (each word of at least two letters, and each part of a compound
  name such as `Smith-Jones` or `O'Brien`) and date of birth are replaced by `[Patient Name]`
  and `[Date of Birth]`;
- identifiers Medorama knows (the connected databases' MRNs and patient IDs, of four or more
  characters that contain a digit) are replaced wherever they appear, whatever their case and
  however their digits are written (`SYN-0004`, `syn4`), and so are recordings'
  paths and the folders they lie in (`[Folder]`), which may be named after the patient;
- telephone numbers, e-mail and web addresses, social security numbers, street addresses and
  MRNs after their label (`MRN: …`) are replaced (`[Phone]`, `[Email]`, `[URL]`, `[SSN]`,
  `[Address]`, `[MRN]`), by rules that miss forms they do not know;
- in text that may be about any patient (a recording's label or error, a plug-in's summary, the
  reasons of a cohort search), every linked patient's names (words of three letters or more)
  are replaced;
- phrases such as "Confirmed by …" or "Signed by …" are replaced by `[Staff Name]`;
- dates in the text are moved back;
- the page marks the text with a warning, because it may still identify someone.

An administrator who prefers to withhold all free text ticks *The text of notes and reports*
in Admin › Privacy Mode; notes, reports and every other free text (any entry's text longer than
40 characters or of several lines, a long written value) then appear as `[Withheld]`, on pages,
in searches (an entry is then found by its name only) and in exports. A short written value (a
result written `positive`) is still shown, through the gate.

A cohort's text criterion, in Privacy Mode (or on a database the user holds without `phi`),
finds a patient only when the text as the session is shown it holds the words: a note that
names a person, an MRN or a date of birth does not tell whose it is. The patient page's search
and its fuzzy matches likewise read the text as shown.

### Searches

In Privacy Mode, a search by name, MRN or date of birth is refused with a message (HTTP 400). A
plain word does not match identifiers behind the scenes either: otherwise the number of results
would reveal whether a person is in a database. Patients are found by their pseudonyms. Outside
Privacy Mode, a search by identifiers looks only in the databases on which the user holds the
`phi` grant. Searches of identifiers by
users who may see them are written to the audit log (`phi_search`), with the text searched as
it was typed: the audit log therefore holds identifiers (a name, an MRN searched), and so do
some of its other entries (the real paths of recordings checked or connected). Only
administrators read it, and in Privacy Mode they see its targets and details as `[Withheld]`;
protect it, and its exports, as patient data.

A recording list's search, in Privacy Mode (or for a database the user holds without `phi`),
reads only what the session is shown (the recording's token, label as shown, format, status,
database), never its path or file name, and sorts by the times as shown.

### What the administrator can change

Admin › Privacy Mode lets an administrator choose whether dates move, whether ages above 89 are
capped and whether free text is withheld, Medorama's offset and `spread_days`. Every change is
written to the audit log (`deid_settings`) with the new settings. Withholding less (unticking a
choice, fewer days of each patient's own, a smaller offset) needs an administrator with the PHI
permission (HTTP 403 otherwise): an administrator who may not see identifiers cannot lift the
protections either. Nor can they reset the password of a user who has the PHI permission (and
sign in as them); a reset by an administrator who has it ends that user's sessions and is
audited (`phi_password_reset`). Some protections are no choice and always apply in Privacy
Mode: patients' names, MRNs, dates of birth and account numbers are withheld, patient IDs and
visits become pseudonyms, file names become tokens, staff names become `[Staff Name]`, and
files the gate cannot de-identify are refused.

**For reviewers:** placeholders and the identifier dictionary (built from each database's
persons file) are in `gate.rs` (`Dict`, `Restricted::json`); identity per database in
`patient/identity.rs` and `patient/list.rs`; dates, ages and the settings
in `deid.rs` (`FIELDS`, `Settings`, `answer_with`, `shift_text`); pseudonyms in
`pseudonyms.rs`. Unit tests in each module; end to end in `medorama/tests/privacy.rs`.

## De-identifying files at the source

Patients' files (ECGs, recordings, DICOM studies, documents) are sent one item at a time,
through `patients/{id}/file/{item}`, and only the files that belong to that item. In Privacy
Mode, the gate decides how each file leaves according to its kind. A file is de-identified as
it is sent; the original on disk is never changed.

### How each kind of file leaves

| Kind of file | In Privacy Mode |
|---|---|
| WFDB header (`.hea`) | Comments that identify the patient (name, ID, MRN, date of birth, address, phone, however their key is written: `Patient_Name:`, `Name=`, `Patient name -`, `Birthdate:`) are removed; the patient's own names and date of birth are replaced in every other comment; staff named in comments become `[Staff Name]`; the base date and dates in comments are moved back; ages above 89 become `90+`; the record's name and its signal files' names are replaced by tokens. |
| WFDB samples (`.dat`) | Sent as they are (they hold only numbers) when a header of the same item names them; any other `.dat` file is refused. |
| WFDB samples in a MATLAB file (`.mat`) | When a header of the same item names it: its first matrix only (the samples WFDB reads; other variables after it are not sent), with the file's descriptive text (the program, the computer and the time it was written) and the matrix's name blanked. A `.mat` file that is not a MATLAB file WFDB reads (level 4, or level 5 not compressed), and any other `.mat` file, is refused. |
| GE MUSE XML (`.xml` with a `RestingECG` element) | The patient's demographics are removed except age (capped) and sex, whatever attributes or spaces their tags hold; the order is removed; staff names become `[Staff Name]` and their IDs are emptied; a patient's name elsewhere becomes `[Patient Name]`; the patient's own names and date of birth are replaced in the statements; "Confirmed by …" phrases are replaced; every date is moved back. A file that still holds a patient's name, ID or date of birth after all this is refused, as is any other XML file. |
| DICOM (`.dcm`, `.dicom`, or a file recognized by its content) | The header is rewritten with the actions of the DICOM confidentiality profile for the attributes listed ([DICOM](#dicom)); the pixel data are copied byte for byte. The file is marked as possibly identifying. |
| EDF, EDF+, BDF, BDF+ (`.edf`, `.bdf`) | The header is de-identified and the texts of annotations are removed; the samples are sent as they are ([EDF and BDF](#edf-and-bdf)). |
| Pictures, videos and PDF documents | Sent as they are, marked as possibly identifying: the viewer's tab shows an amber warning triangle. An SVG picture is sent without its texts (`<text>`, `<title>`, `<desc>`, `<metadata>`, comments); a PDF document of up to 64 MB with its descriptive fields (the information dictionary's author, title, subject, keywords, creator and producer, and the same in uncompressed XMP metadata) blanked in place. What their pages and pixels show is not changed. |
| Anything else | Refused (HTTP 403). |

**File names never leave.** In Privacy Mode, an item's files are named by tokens (`mo-f0000.hea`,
`mo-f0001.dat`), and a request that gives a real file name is not accepted. Patients' files
are sent with `Cache-Control: no-store`: a browser keeps no copy of them.

The files of a database the user holds without `phi` leave the same way outside Privacy Mode
too (de-identified, by tokens), whatever the user's PHI permission.

### DICOM

The rewrite applies the actions of DICOM PS3.15 Table E.1-1, the **Basic Application Level
Confidentiality Profile** (`X`: removed; `Z`: emptied; `D`: a dummy value; `U`: a new UID),
for the attributes listed in `dicom.rs` (`PROFILE`), with the **Retain Longitudinal Temporal
Information with Modified Dates** option (dates moved by the patient's date offset, instead of
removed). It is not a certified implementation of the profile: an attribute of the table that
the list lacks is kept, with its free text scrubbed as below. The De-identification Method
written into the file says so.

| Element | In Privacy Mode |
|---|---|
| Patient's Name | `[Patient Name]` |
| Patient ID | The patient's pseudonym |
| Patient's Birth Date | Emptied |
| Patient's Age | Capped at 90 years when ages are capped |
| Patient's Sex, Size and Weight | Kept |
| Every other element of the patient group (0010) | Removed |
| The clinical trial module (0012) | Removed |
| Other person names (referring, performing and reading physicians, operators, observers) | `[Staff Name]` |
| The attributes of the list: institution, department, station, addresses and phones, device, detector and other equipment IDs, accession, study, order, procedure, admission and visit IDs; descriptions (study, series, requested and performed procedures, derivation, admitting and discharge diagnoses); comments (image, study, acquisition, identifying, performed procedure step, visit); protocol names; structured report text and content (Text Value, Content Sequence); graphic annotations; icon images; digital signatures | Removed (`X`), or emptied (`Z`: accession, institution name, study ID, order numbers) |
| Curves (50xx), overlays (60xx: their bitmaps, labels and comments), the retired results group (4008) | Removed |
| Free text left (SH, LO, ST, LT, UC, UT; in implicit-VR files, elements of unknown VR that hold text) | The patient's own names and date of birth, and the identifiers Medorama knows, replaced; dates in it moved back |
| Private elements | Removed |
| Instance UIDs (study, series, instance, frame of reference, references) | Replaced by `2.25.` and 128 bits of an HMAC-SHA-256 under the gate's secret, so that a series still holds together and nobody without the secret can map them back. The standard's own UIDs (classes, transfer syntaxes) are kept. |
| Dates and times | Moved back; a date and its time together |
| Patient Identity Removed, De-identification Method, Longitudinal Temporal Information Modified | Added (`YES`, the method, `MODIFIED`) |

The file keeps its transfer syntax (explicit or implicit VR little endian, and every
compressed syntax). In implicit VR the file does not say which elements are person names:
the rewrite knows the person names of the standard's dictionary by their tags. Deflated and
big-endian files are refused, because they could not be rewritten safely, and so are files
with sequences nested more than 32 deep. Real files from several vendors and syntaxes (CT in implicit and explicit VR,
JPEG Lossless, JPEG Extended 12-bit, multi-frame JPEG Baseline angiography, panoramic X-ray)
were checked with pydicom after the rewrite: no original identifier and no private element
remain, and the pixels are identical (`medorama/tests/dicom_samples.rs`).

What the pixels show is not changed. Ultrasound images, secondary captures and scanned
documents may carry names burned into the image, so every DICOM file is sent with the
"may contain PHI" mark.

### EDF and BDF

An EDF or BDF header has two identification fields, one for the patient and one for the
recording, and states the recording's start date and time
([edf.md](edf.md#patient-identity-in-edf-files)). In Privacy Mode:

- the patient field becomes `X X X X`, EDF+'s form for an anonymous patient;
- in EDF+ and BDF+ files, the recording field becomes `Startdate dd-MMM-yyyy X X X`, with the
  start date moved back; in plain EDF and BDF files, whose recording field is free text, it
  is left blank;
- the start date and time are moved back. EDF writes two-digit years for 1985 to 2084 only: an
  EDF+ file whose moved date falls outside that range has `yy` in its date field and the full
  year in its recording field, as EDF+ prescribes; a plain EDF file states no start date
  rather than a wrong one;
- in EDF+ and BDF+ files, the texts of annotations are removed. Each data record keeps only
  its time-keeping annotation (its onset), so that the file's timing stays whole;
- the samples are sent as they are.

A file without annotation signals is streamed: only its header is rewritten. A file with
annotation signals is rewritten whole in memory, so EDF+ files with annotations larger than
64 MB are refused in Privacy Mode; such recordings can still be opened in VitalStream. The
technical fields of the signal headers (labels, units, transducer and prefiltering
descriptions) are kept as they are.

The engine and the viewers never put the patient and recording fields into an index or any
answer, and Medorama does not read them: a recording's patient is the one its database names.

### Why some files pass with a warning

Pictures, videos, PDF documents and DICOM pixels are images: the gate cannot read what they
show, and removing burned-in text would require optical character recognition, which
Medorama does not do. Refusing them would make imaging research impossible in Privacy Mode,
so they are sent as they are, with the header `x-medorama-may-contain-phi`, and the page shows
an amber warning triangle on the tab that displays them. Every other kind of file that the gate
cannot de-identify is refused.

**For reviewers:** the file rules are `gate.rs` `FileKind` and `file_kind`, and
`server.rs` `attachment_response`; WFDB headers are `deid.rs` `wfdb_header`; MUSE XML is
`deid.rs` `muse_xml`; DICOM is `dicom.rs`; EDF is `deid.rs` `edf_header` and
`edf_annotations`, called by `server.rs` `edf_deidentified`. File tokens are `gate.rs`
`file_token` and `token_index`.

## The VitalStream viewer

The VitalStream waveform viewer runs as a separate program on the server, reachable only
through Medorama's **gateway** (`/viewer/`). The viewer itself never reads names or dates of
birth from any recording. In Privacy Mode, the gateway adds the following rules:

- recordings are opened by token (`mo-rec:<n>`), never by path; a path sent by the page is
  refused;
- browsing and scanning folders is refused, because folder and file names may identify;
- the times the page sends are moved forward before they reach the viewer, and every time in
  the viewer's answers is moved back, by the recording's patient's date offset;
- a dataset's path and name are replaced by its token and `[Recording]`;
- every answer then passes the gate's last layer, like any other answer.

EDF+ annotations appear as events in VitalStream's Annotations lane. Because they are free text,
the gateway treats them as Medorama treats clinical notes. When Privacy Mode withholds notes'
text, each annotation is replaced by `[Annotation]`. Otherwise the patient's own names and date
of birth are replaced by placeholders, and the gate's last layer then replaces the identifiers
Medorama knows and moves dates back. Like notes, such text may still identify someone in ways
no rule recognizes; see [What Privacy Mode cannot do](#what-privacy-mode-cannot-do).

**For reviewers:** `gateway.rs` (`handle`, `shift_query`, `gate_annotations`); tested with a real
viewer in `medorama/tests/gateway.rs`, and `gate_annotations` by its unit test
`annotations_in_privacy_mode`.

## Exports and plug-in actions

- **Exports** made in Privacy Mode (cohort members, definitions, members' data as CSV or Excel
  files) pass the same gate. They have the same columns as identified exports,
  with identifiers as placeholders, patients named by pseudonym and dates moved back. Exports by
  users who see identifiers are written to the audit log (`phi_export`).
- **Plug-in actions** (programs an administrator installs to analyze recordings) receive a job
  description with the recording's real path, an output folder, and its patient's
  pseudonym ([actions.md](actions.md)), and they read the recording's files as they are on
  disk: the file name, the folder and the files' own headers may identify the patient, and the
  pseudonym does not change that. A plug-in is therefore trusted code with access to patient
  data, as the server is: install only plug-ins reviewed for it. They do not receive the
  server's secrets in their environment
  ([security.md](security.md#13-background-jobs-and-plug-in-actions)). What they write back (a
  summary, an error) is shown through the gate: dates under keys of their own are moved too,
  and every linked patient's names are replaced.

## Connected databases

Every database is a connected database (an OMOP instance, a data warehouse read through SQL
views, a SQLite file made by db-builder), read in place, read-only, and its data leave through
the same gate.

- **Only canonical columns are read.** Medorama reads a connected database through views of fixed
  columns (persons, visits, entries, and recordings), each column with a privacy class: key,
  identifier, date, free text, staff or clinical value. A view cannot add a field that the gate
  does not know ([databases.md](databases.md)). Columns that a db-builder database keeps beside
  them (`extras`) are not read.
- **Persons are linked to patients by exact identity.** A person whose (issuer, identifier) pair
  equals one in another database is the same patient; the identifiers are kept in the database's
  encrypted persons file, where the gate's dictionary reads them, and as keyed hashes in the
  control store. Persons leave as pseudonyms; their keys in the database never leave.
- **Visits and entries are named by tokens.** A visit is named by a keyed token
  (`enc:<database>:<token>`) and shown by its pseudonym; an entry by a keyed token
  (`live:<database>:<token>`) that holds neither its key nor its time. Dates move back with the
  rest, text follows the rule for notes, and authors are treated as staff.
- **Identifiers per database.** A database's identifiers reach only users with the PHI
  permission and the `phi` grant on it, outside Privacy Mode. A patient in two databases shows
  only the identity held by the databases granted `phi`.
- **Searches** of identifiers need the PHI permission and look only in the databases granted
  `phi`.

**For reviewers:** `sources/` (read-only sessions, views), `persons.rs` (the patient index) and
`patient/items.rs`; tested in `medorama/tests/privacy.rs` (over a db-builder database) and
`medorama/tests/databases.rs`.

## Failing closed

Most answers are de-identified by the code that builds them, because that code knows what each
value is. A last layer of the server then enforces the rule for every answer of a session in
Privacy Mode:

- a JSON answer passes through the gate there, whatever the endpoint;
- a file passes only if the gate cleared it;
- any other answer that carries data is refused (HTTP 403), and the refusal is written to the
  server's error output.

The pages and scripts of the web interface, redirects and error messages hold no patient data
and pass unchanged. As a result, a new endpoint whose author forgets the gate cannot leak
identifiers: its answer is refused or de-identified. This is why the gate is said to fail
closed. In the same spirit, a session in Privacy Mode is handled as a user without the PHI
permission, so a forgotten permission check also errs on the side of withholding.

**For reviewers:** `server.rs` `privacy` (the last layer) and `cleared`, `may_hold_phi`
(the marks set by handlers); the internal header `x-medorama-gate` is removed before an answer
leaves.

## How the design is verified

- **End to end.** `medorama/tests/privacy.rs` connects a db-builder database seeded with
  identifiers that cannot appear by chance: a name, an MRN, a patient ID, the database's key for
  a visit, a date of birth, staff names, and real dates in the forms that tables, DICOM, MUSE,
  WFDB, EDF and EDF+ write them. They are placed in every format Medorama serves: results,
  notes, a WFDB record, GE MUSE XML, DICOM, an EDF+ file, a picture, a text file and an ICU
  recording. The test then requests every page's answers, every entry's detail, every file and
  its parts, and every cohort export, as a user without the PHI permission and as an
  administrator who switched Privacy Mode on. It asserts that no byte of any answer holds any
  of the seeded identifiers, that no JSON number is a real time, that identifier searches are
  refused or find nobody, that placeholders and pseudonyms appear, that the text file is
  refused, and that pictures and DICOM files are marked. It also checks that the identified
  answers do hold the identifiers, so that the test would fail if it checked nothing.
- **The viewer.** `medorama/tests/gateway.rs` runs a real VitalStream viewer behind the
  gateway, in Privacy Mode too.
- **Grants, identity and removal.** `medorama/tests/databases.rs` checks that grants decide what
  a user sees, that equal identifiers link one patient and conflicting ones are never merged,
  and that a removed database leaves no trace (its workspace, cohort rows, patients, search
  results), also after a removal a crash interrupted.
- **Each de-identifier.** Unit tests in `gate.rs`, `deid.rs` (including real EDF test files),
  `dicom.rs` and `pseudonyms.rs`; `medorama/tests/dicom_samples.rs` checks real DICOM files
  with pydicom.

The commands that run these tests are listed in
[security.md, Verifying these claims](security.md#18-verifying-these-claims).

## What Privacy Mode cannot do

Privacy Mode reduces what users see; it does not make data anonymous. Reviewers and users should
know its limits.

- **Burned-in text.** Names written into pictures, scanned PDFs, video frames and DICOM pixels
  are not removed. These files are marked with the amber warning instead.
- **Free text.** Notes, reports and EDF+ annotations may name the patient, relatives or staff in
  ways no rule recognizes. Notes are marked; an administrator can withhold the text of notes
  entirely, and the same setting withholds EDF+ annotation texts in VitalStream.
- **Quasi-identifiers.** Sex, a capped age, rare diagnoses, units and beds, small cohorts and
  exact intervals can single out a person when combined with outside knowledge.
- **Date offsets.** Learning one real date of a patient reveals that patient's offset, and every
  other patient's to within `spread_days`.
- **Not Safe Harbor.** Because dates are shifted rather than removed, data viewed or exported in
  Privacy Mode are not de-identified under HIPAA's Safe Harbor method. They suit use as a
  limited data set under a data use agreement, or under an expert determination
  ([security.md, Regulatory mapping](security.md#15-regulatory-mapping)).
- **A grant covers a whole database.** Medorama has no per-patient or per-study access lists
  within a database: a user granted a database may see all of its patients, as Privacy Mode,
  the `phi` grant and the PHI permission allow. Separate databases, granted separately, serve
  separate studies.
- **Not everything is encrypted.** The files plug-ins write are not
  encrypted in this version, and a backup made before a removal still holds the removed data.

The deploying institution remains responsible for the rest: who receives accounts and the PHI
permission, which databases are connected and to whom they are granted, how the server, the
master key and the backups are protected, the identity rules of each database, and how exports
are used and shared. The [deployment checklist](security.md#16-deployment-checklist)
lists these duties.

## The repository holds no patient data

Medorama's source repository contains no patient data. Tests and demonstrations use synthetic
data only, or public data sets kept outside the repository. A pre-commit hook
(`tools/git-hooks`) refuses recordings, data files, notebooks with outputs and real recording
names. The rules are in [PRIVACY.md](../PRIVACY.md).
