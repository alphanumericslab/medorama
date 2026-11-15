#!/usr/bin/env python3
"""The architecture and security diagrams of docs/ (SVG), as code.

    python3 docs/images/diagrams.py        # writes docs/images/*.svg

Plain SVG with no dependencies, so that the diagrams render on GitHub, can be reviewed as text
in a pull request, and stay consistent: one palette (Okabe-Ito, colour-blind safe; no red,
which the product keeps for alarms), one font, one box and arrow style.  Edit the drawings
below, run this script, commit both.
"""

from pathlib import Path
from xml.sax.saxutils import escape

OUT = Path(__file__).resolve().parent
FONT = "Inter, -apple-system, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif"

# fill, stroke, text: by kind of thing
STYLE = {
    'medorama': ('#E8F1FA', '#0072B2', '#0B3C5D'),      # Medorama (blue)
    'gate': ('#FDF1DC', '#B97A00', '#5C3D00'),      # the privacy gate (amber)
    'viewer': ('#E4F4EE', '#009E73', '#0B4A38'),    # viewers (green)
    'store': ('#F2F3F5', '#6B7280', '#1F2937'),     # storage (grey)
    'phi': ('#F6E8F1', '#A8507E', '#4A1D36'),       # holds identifiers (purple)
    'outside': ('#FFFFFF', '#9CA3AF', '#374151'),   # outside the host
    'person': ('#FFFFFF', '#6B7280', '#1F2937'),
}


class Svg:
    def __init__(self, w, h, title, desc):
        self.w, self.h = w, h
        self.parts = [
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" role="img" aria-labelledby="t d">',
            f'<title id="t">{escape(title)}</title><desc id="d">{escape(desc)}</desc>',
            '<defs><marker id="a" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">'
            '<path d="M0 0L10 5L0 10z" fill="#4B5563"/></marker>'
            '<marker id="ag" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">'
            '<path d="M0 0L10 5L0 10z" fill="#B97A00"/></marker></defs>',
            f'<rect x="0" y="0" width="{w}" height="{h}" rx="14" fill="#FFFFFF"/>',
            f'<g font-family="{FONT}" font-size="13" fill="#1F2937">',
        ]

    def text(self, x, y, s, size=13, weight=400, fill='#1F2937', anchor='start', italic=False):
        style = ' font-style="italic"' if italic else ''
        self.parts.append(
            f'<text x="{x}" y="{y}" font-size="{size}" font-weight="{weight}" fill="{fill}" text-anchor="{anchor}"{style}>{escape(s)}</text>'
        )

    def box(self, x, y, w, h, kind, title, lines=(), title_size=13.5, dashed=False):
        fill, stroke, ink = STYLE[kind]
        dash = ' stroke-dasharray="5 4"' if dashed else ''
        self.parts.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="9" fill="{fill}" stroke="{stroke}" stroke-width="1.4"{dash}/>')
        self.text(x + 12, y + 21, title, size=title_size, weight=650, fill=ink)
        for i, line in enumerate(lines):
            indent = (len(line) - len(line.lstrip(' '))) // 4 * 16
            self.text(x + 12 + indent, y + 40 + 16.5 * i, line.lstrip(' '), size=11.5, fill='#374151')

    def zone(self, x, y, w, h, label):
        self.parts.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="12" fill="none" stroke="#9CA3AF" stroke-width="1.2" stroke-dasharray="7 5"/>')
        self.text(x + 12, y + 18, label, size=11.5, weight=600, fill='#6B7280')

    def arrow(self, pts, label=None, at=0.5, gate=False, both=False, dy=-6, anchor='middle'):
        d = 'M' + ' L'.join(f'{x} {y}' for x, y in pts)
        colour, marker = ('#B97A00', 'ag') if gate else ('#4B5563', 'a')
        start = f' marker-start="url(#{marker})"' if both else ''
        self.parts.append(f'<path d="{d}" fill="none" stroke="{colour}" stroke-width="1.5" marker-end="url(#{marker})"{start}/>')
        if label:
            (x0, y0), (x1, y1) = pts[0], pts[-1]
            if len(pts) > 2:
                (x0, y0), (x1, y1) = pts[len(pts) // 2 - 1], pts[len(pts) // 2]
            self.text(x0 + (x1 - x0) * at, y0 + (y1 - y0) * at + dy, label, size=11, fill='#4B5563', anchor=anchor)

    def save(self, name):
        self.parts.append('</g></svg>')
        (OUT / name).write_text('\n'.join(self.parts) + '\n', encoding='utf-8')
        print('wrote', OUT / name)


def system_context():
    s = Svg(1000, 764, 'Medorama: system context and trust boundaries',
            'People use a browser; the browser reaches the Medorama server over TLS or an SSH tunnel. On the server host, '
            'the server, its VitalStream viewer and its plug-ins run under one account, with the control store, the '
            'encrypted workspace and the master key. The connected databases are outside the host and only read; the '
            'master key may come from a key manager.')
    s.text(24, 34, 'System context and trust boundaries', size=17, weight=700)
    s.text(24, 54, 'Every request crosses one boundary into the server, where sign-in, roles, grants and the privacy gate decide what leaves.', size=12, fill='#4B5563')
    # people
    for i, (who, what) in enumerate([('Researcher', 'cohorts, data, viewers'), ('Clinician', 'a patient’s journey'), ('Data manager (admin)', 'databases, users, grants')]):
        s.box(24, 90 + i * 74, 170, 58, 'person', who, [what])
    # browser
    s.zone(214, 76, 220, 240, 'User’s computer (untrusted)')
    s.box(228, 104, 192, 190, 'outside', 'Web browser', ['Medorama page', 'ECG, PACS, VitalStream', 'viewers (static pages)', '', 'stores nothing; shows', 'what the server sends'])
    for i in range(3):
        s.arrow([(194, 119 + i * 74), (228, 119 + i * 74)])
    # network
    s.arrow([(420, 200), (548, 200)], both=True)
    for i, line in enumerate(['HTTPS through', 'a reverse proxy,', 'or an SSH tunnel']):
        s.text(484, 166 + 14 * i, line, size=11, fill='#4B5563', anchor='middle')
    # host
    s.zone(540, 76, 440, 520, 'Server host (trusted; Medorama’s OS account)')
    s.box(556, 104, 408, 150, 'medorama', 'Medorama server  (127.0.0.1:8090 by default)',
          ['sign-in (Argon2id), sessions, roles, grants', 'API, patient pages, cohorts, databases', 'data keys unwrapped only for granted requests', 'privacy gate; audit log, job queue, workers'])
    s.box(556, 270, 196, 110, 'viewer', 'VitalStream viewer', ['127.0.0.1, private port,', 'reached only through', 'Medorama’s gateway;', 'add-ons read other formats'])
    s.box(768, 270, 196, 110, 'medorama', 'Plug-in actions', ['separate processes,', 'own process group,', 'timeout; get pseudonyms', 'and file paths only'])
    s.arrow([(654, 254), (654, 270)])
    s.arrow([(866, 254), (866, 270)])
    s.box(556, 398, 196, 90, 'store', 'Control store', ['SQLite (owner-only), or', 'PostgreSQL (TLS, verified);', 'no identifiers: keyed hashes'])
    s.box(556, 500, 196, 84, 'phi', 'Workspace', ['SQLite files, encrypted', '(SQLCipher), a key per', 'database and per cohort'])
    s.box(768, 398, 196, 90, 'store', 'Index, plug-in output', ['folders, not encrypted yet:', 'waveform indexes, files', 'plug-ins write'])
    s.box(768, 500, 196, 84, 'store', 'Master key file', ['the default, outside the', 'workspace; or the key', 'from a key manager'])
    # only the server writes the control store and the workspace
    s.arrow([(556, 236), (547, 236), (547, 443), (556, 443)])
    s.arrow([(547, 443), (547, 542), (556, 542)])
    # outside the host
    s.box(556, 642, 196, 88, 'outside', 'Connected databases', ['OMOP, SQL views, db-builder', 'SQLite; the recording files', 'they name. Only read.'], dashed=True)
    s.box(768, 642, 196, 88, 'outside', 'Key manager (optional)', ['keyring, Vault, a cloud', 'KMS: gives the master key', '(master_key_command)'], dashed=True)
    s.arrow([(654, 642), (654, 624), (760, 624), (760, 254)])
    s.text(664, 618, 'read only', size=11, fill='#4B5563')
    s.arrow([(866, 642), (866, 584)])
    s.text(24, 340, 'Inputs', size=13, weight=650)
    for i, line in enumerate([
        'Connected databases, read in place, never',
        'written: OMOP, any SQL database through views,',
        'SQLite databases built by db-builder; and the',
        'recording files they name. Nothing is imported.',
    ]):
        s.text(24, 362 + 17 * i, line, size=11.5, fill='#374151')
    s.text(24, 460, 'Outputs to people', size=13, weight=650)
    for i, line in enumerate([
        'Identified: users with the PHI permission and',
        'the phi grant on the database, Privacy Mode off.',
        'Privacy Mode (default for everyone but admins):',
        'pseudonyms, placeholders, dates moved back;',
        'files de-identified or marked.',
    ]):
        s.text(24, 482 + 17 * i, line, size=11.5, fill='#374151')
    s.text(24, 750, 'Dashed: outside the host. The server, the viewer and the plug-ins read the connected databases; nothing writes to them.', size=11.5, fill='#374151')
    s.save('system-context.svg')


def components():
    s = Svg(1000, 824, 'Medorama server: components and request path',
            'Requests pass the security headers and request check, the privacy layer (sign-in, Privacy Mode decision, '
            'the gate on the way out), then the handlers, which check the role and the grants, unwrap the data keys they '
            'need, and use the core modules over the control store, the workspace and the connected databases.')
    s.text(24, 34, 'Inside the server: one request’s path', size=17, weight=700)
    s.text(24, 54, 'Layers are axum middleware; the privacy layer wraps every route, so every answer passes it on the way out.', size=12, fill='#4B5563')
    s.box(24, 84, 952, 56, 'outside', 'guard  (server.rs)', ['security headers on every answer (CSP, frame rules, nosniff, no-referrer, no-store on the API); X-Medorama: 1 required on every changing API request'])
    s.box(24, 152, 952, 72, 'gate', 'privacy layer  (server.rs privacy, signin.rs, gate.rs)', ['in: signin.rs identify finds the user once; Privacy Mode unless the user has the PHI permission and turned it off (POST privacy)',
                                                                                                 'out, in Privacy Mode: JSON through the gate; files only if the gate cleared them; anything else refused (fail closed)'])
    cols = [
        ('API  /api/v1', ['patients, report, detail, files', 'cohorts, search across databases', 'databases: connect, check,', '    refresh, grants, remove', 'users, audit (admins)']),
        ('Gateway  /viewer/', ['allow-list of viewer API v1;', 'only recordings of databases', 'the user may view; Privacy', 'Mode: tokens, times', 'translated, no folders']),
        ('Pages  /, /viewers/', ['the Medorama page and the', 'ECG and PACS viewers:', 'static files, no data,', 'embedded in the binary']),
        ('Command line  medorama', ['init, serve, databases, keys,', 'run, work, user, audit;', 'the person at the terminal', 'holds every grant']),
    ]
    for i, (t, lines) in enumerate(cols):
        s.box(24 + i * 240, 240, 232, 126, 'medorama', t, lines)
    for i in range(4):
        s.arrow([(140 + i * 240, 224), (140 + i * 240, 240)])
        s.arrow([(140 + i * 240, 366), (140 + i * 240, 382)])
    s.box(24, 382, 952, 66, 'medorama', 'Every handler  (grants.rs, workspace.rs)', [
        'checks the role and the grants the request needs (search < view < phi < manage on a database; view < edit < manage on a cohort),',
        'then unwraps the data key of each database or cohort it reads, only for a user who holds a grant on it'], title_size=13)
    mods = [
        ('gate.rs', ['dictionary of identifiers,', 'placeholders, file tokens,', 'table cells, notes']),
        ('deid.rs', ['dates moved back (per', 'patient), ages 90+; WFDB,', 'EDF headers; MUSE XML']),
        ('dicom.rs', ['DICOM headers (PS3.15', 'Basic Profile), pixels', 'copied byte for byte']),
        ('pseudonyms.rs', ['MO-XXXX-XXXX, random,', 'with a check character,', 'never derived from an ID']),
        ('databases.rs, sources/', ['connect, check, refresh,', 'remove; OMOP, views,', 'db-builder: read only']),
        ('persons.rs, variables.rs', ['exact (issuer, identifier)', 'links, conflicts; what', 'each database holds']),
        ('patient/, cohorts/', ['the patient page, read live;', 'criteria sent to the', 'databases that may hold them']),
        ('recordings.rs, jobs.rs', ['recordings’ state, indexing,', 'actions as separate', 'processes (pseudonyms only)']),
        ('signin.rs, users.rs', ['password or proxy sign-in;', 'Argon2id, sessions, roles,', 'the PHI permission']),
        ('grants.rs', ['grants per database and', 'per cohort; the admin who', 'connects gets every grant']),
        ('workspace.rs', ['data keys wrapped by the', 'master key (AES-256-GCM);', 'SQLCipher files']),
        ('audit.rs', ['who did what, when;', 'sealed every minute by', 'a SHA-256 chain']),
    ]
    for i, (t, lines) in enumerate(mods):
        x = 24 + (i % 4) * 240
        y = 464 + (i // 4) * 96
        s.box(x, y, 232, 84, 'gate' if t in ('gate.rs', 'deid.rs', 'dicom.rs', 'pseudonyms.rs') else 'medorama', t, lines)
    s.box(24, 756, 300, 52, 'store', 'Control store  (db.rs, sql/, schema/)', ['SQLite or PostgreSQL; no identifiers'], title_size=12.5)
    s.box(350, 756, 300, 52, 'phi', 'Workspace  (workspace.rs)', ['one folder per database, a file per cohort'], title_size=12.5)
    s.box(676, 756, 300, 52, 'outside', 'Connected databases  (sources/)', ['read in place, read-only sessions'], title_size=12.5, dashed=True)
    s.save('components.svg')


def privacy_gate():
    s = Svg(1000, 552, 'The privacy gate: how an answer leaves in Privacy Mode',
            'The session decides the mode; in Privacy Mode JSON answers have identifiers replaced, dates moved back and '
            'patient names removed from text; files are de-identified by kind, passed with a warning, or refused.')
    s.text(24, 34, 'The privacy gate: what leaves in Privacy Mode', size=17, weight=700)
    s.text(24, 54, 'Identifiers stay in the connected databases and the encrypted workspace; the gate decides, at sending, what leaves.', size=12, fill='#4B5563')
    s.box(24, 84, 200, 98, 'outside', 'Request', ['the signed-in user (signin.rs)', '+ Privacy Mode choice', '(set by POST privacy,', 'audited)'])
    s.box(260, 84, 236, 98, 'gate', 'Mode', ['no PHI permission: always on', 'PHI permission: on by default', '(off by default for admins)', 'answered as without PHI'])
    s.box(532, 84, 200, 98, 'medorama', 'Handler', ['reads the databases the', 'user is granted; files', 'named by tokens; searches', 'of identifiers refused'])
    s.box(768, 84, 208, 98, 'outside', 'Browser', ['pseudonyms,', '[Patient Name], [MRN] …', 'dates moved back'])
    s.arrow([(224, 133), (260, 133)])
    s.arrow([(496, 133), (532, 133)])
    s.text(744, 214, 'the answer, on its way out', size=12, weight=600, fill='#5C3D00', anchor='middle')
    s.box(24, 230, 300, 186, 'gate', 'JSON answers', ['1  identifiers replaced (the dictionary):', '    patient IDs → MO- pseudonyms', '    visit IDs → MO-E- pseudonyms', '    MRNs, CSNs → [MRN], [CSN]', '    recordings’ paths → mo-rec tokens', '2  dates and times moved back (per patient)', '3  ages above 89 → 90+; staff → [Staff Name]', '4  the patient’s own names in text → placeholder'])
    s.box(348, 230, 300, 186, 'gate', 'Files, by kind', ['WFDB header: identifying comments out,', '    dates moved, file names → tokens', 'GE MUSE XML: demographics, staff,', '    order out; dates moved', 'DICOM: header rewritten (PS3.15 Basic', '    Profile), pixels as they are', 'EDF, BDF: header de-identified,', '    annotation texts out', 'WFDB samples: numbers only, as they are'])
    s.box(672, 230, 304, 186, 'gate', 'Passed with a warning, or refused', ['pictures, videos, PDF: as they are,', '    marked x-medorama-may-contain-phi', '    (the page shows an amber warning triangle)', 'DICOM: marked too (burned-in text)', 'any other kind of file: refused', 'notes’ text: through the gate, marked', '    (free text may still identify)'])
    s.box(24, 436, 952, 100, 'gate', 'Last layer: fail closed', ['every answer of a Privacy Mode session passes server.rs privacy: JSON through the gate; a file only if the gate cleared it;',
                                                               'any other answer carrying data is refused and logged. A new endpoint that forgets the gate cannot leak: its answer is refused.',
                                                               'Proof: tests/privacy.rs seeds identifiers in every format and checks no byte of any page, file or export holds them.'])
    s.arrow([(614, 182), (614, 198), (174, 198), (174, 230)], gate=True)
    s.arrow([(614, 198), (498, 198), (498, 230)], gate=True)
    s.arrow([(614, 198), (824, 198), (824, 230)], gate=True)
    s.save('privacy-gate.svg')


def data_in():
    s = Svg(1000, 600, 'How data come in: databases, connected',
            'Medorama imports nothing. Tables, known exports and folders of recordings become a SQLite database with '
            'db-builder, a separate command; OMOP databases and any SQL database through views are connected directly. '
            'Medorama reads every connected database in place, read-only, and the recording files it names where they lie.')
    s.text(24, 34, 'How data come in: databases, connected', size=17, weight=700)
    s.text(24, 54, 'Medorama imports nothing. A data manager builds a database with db-builder, or a site writes views; an admin connects it.', size=12, fill='#4B5563')
    for x, label in [(24, 'What a site has'), (276, 'Built by'), (528, 'A database'), (776, 'Medorama')]:
        s.text(x, 84, label, size=12, weight=650, fill='#6B7280')
    s.box(24, 96, 220, 80, 'outside', 'Tables', ['CSV, Excel, OpenDocument,', 'Parquet'])
    s.box(24, 190, 220, 80, 'outside', 'Known exports', ['GE MUSE XML, CLIF folders,', 'a site’s export (TOML)'])
    s.box(24, 284, 220, 80, 'outside', 'Folders of recordings', ['WFDB, EDF, ECGs, DICOM,', 'pictures, PDF'])
    s.box(276, 96, 220, 268, 'medorama', 'db-builder', ['a separate command, run', 'by a data manager:', '', 'recognizes the columns', '(docs/importing.md)', 'names each patient by', '    (issuer, identifier)', 'keeps every other column', 'lists every file a row', '    names as a recording', 'writes only its output'])
    for y in (136, 230, 324):
        s.arrow([(244, y), (276, y)])
    s.box(528, 96, 208, 120, 'store', 'A SQLite database', ['persons, visits, entries,', 'recordings, meta (version):', 'an open schema; may be', 'encrypted (--encrypt)'])
    s.arrow([(496, 156), (528, 156)])
    s.box(528, 232, 208, 92, 'outside', 'An OMOP database', ['OMOP CDM 5.3 or 5.4,', 'PostgreSQL or SQLite:', 'views built in'], dashed=True)
    s.box(528, 340, 208, 108, 'outside', 'Any SQL database', ['views written by the site:', 'persons, visits, entries,', 'recordings, version'], dashed=True)
    s.box(776, 96, 200, 352, 'medorama', 'Connected', ['Databases › Connect a', 'database, or medorama', 'databases connect', '', 'read in place, through', 'read-only sessions;', 'persons linked by exact', '(issuer, identifier) pairs;', 'variables and recordings', 'read; checked every', 'minute, read again when', 'its version changes'])
    for y in (156, 278, 394):
        s.arrow([(736, y), (776, y)])
    s.box(24, 476, 952, 68, 'outside', 'Recording files, ECGs, images, documents',
          ['stay where they lie, inside data_roots; a database’s recordings rows name them, with their patient. Medorama reads them, never copies or writes them.'], dashed=True)
    s.arrow([(876, 476), (876, 448)])
    s.parts.append('<path d="M134 364 L134 476" fill="none" stroke="#9CA3AF" stroke-width="1.4" stroke-dasharray="5 4"/>')
    s.text(144, 424, 'the same files: listed, never copied', size=11, fill='#4B5563')
    s.text(24, 572, 'Dashed: kept by the site. No part of Medorama writes into a connected database; requirements on one: docs/databases.md.', size=11.5, fill='#374151')
    s.save('data-in.svg')


def sources():
    s = Svg(1000, 640, 'Medorama: connected databases, the control store and the workspace',
            'Medorama reads each connected database in place. The patient index links each database\'s persons to patients '
            'by exact (issuer, identifier) pairs. The control store keeps patient numbers, pseudonyms and keyed hashes of the '
            'pairs, never identifiers; each database\'s persons as linked, its variables and recordings are kept in its own '
            'folder of the workspace, encrypted with its data key, which the master key wraps.')
    s.text(24, 34, 'Connected databases, the control store and the workspace', size=17, weight=700)
    s.text(24, 54, 'One patient, one pseudonym, across every database; identifiers only in the databases and the encrypted workspace.', size=12, fill='#4B5563')
    dbs = [('An OMOP database', 'views built into Medorama'), ('A db-builder database', 'its tables are the views'), ('Any SQL database', 'views written by the site')]
    for i, (t, line) in enumerate(dbs):
        s.box(24 + i * 328, 80, 296, 64, 'outside', t, [line + '; only read'], dashed=True)
    s.zone(24, 164, 952, 172, 'Medorama server')
    s.arrow([(172, 144), (172, 196)])
    s.arrow([(500, 144), (500, 196)])
    s.arrow([(828, 144), (828, 154), (600, 154), (600, 196)])
    s.box(40, 196, 580, 124, 'medorama', 'Patient index  (persons.rs)', [
        'each person’s pairs: the patient ID under its issuer (patient_id_issuer, or the',
        '    database’s issuer setting, else the database itself), the MRN under mrn:<authority>',
        'a pair equal in two databases: one patient, one pseudonym (exact joins only)',
        'pairs that name two patients: a conflict, listed, never linked or merged',
    ])
    s.box(636, 196, 324, 124, 'medorama', 'Keys  (workspace.rs)', ['a data key per database and per', 'cohort, wrapped by the master key', '(AES-256-GCM); unwrapped only for', 'a request whose user holds a grant'])
    s.box(24, 384, 300, 184, 'store', 'Control store', ['SQLite (one server) or PostgreSQL', 'patients: number, MO- pseudonym;', '    no identifier', 'patient_keys: a keyed hash of each', '    pair, and its patient', 'databases, grants, wrapped keys,', 'cohort definitions, jobs, audit'])
    s.box(356, 384, 400, 184, 'phi', 'Workspace  (encrypted, SQLCipher)', ['db-0001/   one folder per connected database', '    persons.sqlite: identifiers, names, birth date, sex', '    variables.sqlite, recordings.sqlite', '    results-<action>.sqlite', 'cohorts/c-0001.sqlite: members, provenance', 'each file with its owner’s data key', 'index/: index format 2, not encrypted yet'])
    s.box(788, 384, 188, 184, 'outside', 'Master key', ['outside the workspace:', '$MEDORAMA_MASTER_KEY,', 'master_key_command', '(keyring, Vault, KMS),', 'or master_key_file'], dashed=True)
    s.arrow([(172, 320), (172, 384)])
    s.text(182, 356, 'keyed hashes, pseudonyms', size=11, fill='#4B5563')
    s.arrow([(500, 320), (500, 384)])
    s.text(510, 356, 'identifiers, as linked', size=11, fill='#4B5563')
    s.arrow([(700, 320), (700, 384)])
    s.text(694, 356, 'opens', size=11, fill='#4B5563', anchor='end')
    s.arrow([(882, 384), (882, 320)])
    s.text(892, 356, 'wraps', size=11, fill='#4B5563')
    s.text(24, 598, 'Removing a database destroys its data key, deletes its workspace folder, its keyed hashes and every cohort member it put there.', size=11.5, fill='#374151')
    s.text(24, 616, 'The database itself is never touched. Design: docs/design/databases-only.md and docs/design/sources.md.', size=11.5, fill='#374151')
    s.save('sources.svg')


def architecture():
    s = Svg(1000, 760, 'Medorama: architecture at a glance',
            'People use a browser; requests pass the institution\'s front door (TLS, optionally its single sign-on), then '
            'Medorama\'s sign-in, roles and grants, and handlers, which unwrap the data keys of the databases they read; every '
            'answer leaves through the privacy gate. Readers turn each format into one of four data models. Medorama keeps a '
            'control store with no identifiers and an encrypted workspace, and only reads the connected databases.')
    s.text(24, 34, 'Medorama at a glance', size=17, weight=700)
    s.text(24, 54, 'One server, one way in, one way out: sign-in and grants decide who may read what, the privacy gate decides what leaves.', size=12, fill='#4B5563')
    s.box(24, 80, 220, 76, 'person', 'People', ['researchers, clinicians,', 'data managers, admins'])
    s.box(276, 80, 220, 76, 'outside', 'Web browser', ['the Medorama page and viewers:', 'static pages; stores no data'])
    s.box(528, 80, 448, 76, 'outside', 'The institution’s front door', ['HTTPS (a reverse proxy), and where required its sign-in:', 'single sign-on, MFA, VPN — the part each institution adapts'], dashed=True)
    s.arrow([(244, 118), (276, 118)])
    s.arrow([(496, 118), (528, 118)], both=True)
    s.zone(24, 178, 952, 346, 'Medorama server')
    w = 218
    boxes = [
        ('1  Sign-in  (signin.rs)', ['who is asking: a password,', 'or the name the proxy sends']),
        ('2  Roles and grants', ['role, PHI permission (users.rs);', 'grants per database (grants.rs)']),
        ('3  Handlers', ['API, viewer gateway, pages:', 'databases, cohorts, patients']),
        ('4  Data keys  (workspace.rs)', ['a database’s key unwrapped', 'only for a granted request']),
    ]
    for i, (t, lines) in enumerate(boxes):
        s.box(40 + i * (w + 16), 206, w, 80, 'medorama', t, lines, title_size=13)
        if i:
            s.arrow([(40 + i * (w + 16) - 16, 246), (40 + i * (w + 16), 246)])
    s.arrow([(752, 156), (752, 168), (149, 168), (149, 206)])
    s.box(40, 302, 920, 64, 'gate', '5  The privacy gate  (gate.rs, deid.rs, dicom.rs)', ['every answer, file and export of a session in Privacy Mode leaves through it: identifiers replaced, dates moved back,',
                                                                                         'files de-identified at the source; an answer it did not clear is refused (fail closed)'])
    s.arrow([(617, 286), (617, 302)], gate=True)
    s.box(40, 382, 440, 126, 'medorama', 'Readers: one per format', ['WFDB, EDF, ECG and DICOM files; connected databases', '(OMOP, SQL views, db-builder SQLite); licensed formats', 'through add-ons', 'Past the reader, every format is one of four data models:', 'recordings, ECGs, images, database tables'])
    s.box(496, 382, 220, 126, 'viewer', 'VitalStream viewer', ['waveforms; a private port', 'on 127.0.0.1, reached only', 'through Medorama’s', 'gateway'])
    s.box(732, 382, 228, 126, 'medorama', 'Jobs, plug-ins, add-ons', ['indexing and actions;', 'plug-ins and adapters run as', 'separate programs and', 'get pseudonyms, not PHI'])
    s.box(24, 548, 300, 122, 'store', 'Control store', ['SQLite or PostgreSQL: users, grants,', 'the list of databases, wrapped keys,', 'keyed hashes of identifiers (never', 'identifiers), cohorts, jobs, audit'])
    s.box(350, 548, 300, 122, 'phi', 'Workspace  (encrypted)', ['one folder per database (its persons', 'as linked, variables, recordings,', 'results), one file per cohort; index', 'folders are not encrypted yet'])
    s.box(676, 548, 300, 122, 'outside', 'Connected databases  (read only)', ['OMOP, any SQL database through', 'views, db-builder SQLite; the', 'recording files they name, read', 'where they lie'], dashed=True)
    s.arrow([(174, 524), (174, 548)], both=True)
    s.text(184, 540, 'read, write', size=11, fill='#4B5563')
    s.arrow([(500, 524), (500, 548)], both=True)
    s.text(510, 540, 'read, write', size=11, fill='#4B5563')
    s.arrow([(826, 548), (826, 524)])
    s.text(836, 540, 'read only', size=11, fill='#4B5563')
    s.text(24, 700, 'Numbers: a request’s path. Dashed: outside Medorama. The master key that wraps the data keys is kept outside the workspace', size=11.5, fill='#374151')
    s.text(24, 718, '(an environment variable, a key file, or a command: a keyring, Vault, a KMS). The server runs as one operating-system account and', size=11.5, fill='#374151')
    s.text(24, 736, 'listens on 127.0.0.1 by default. An institution adapts its front door, and at most one module of Medorama (signin.rs).', size=11.5, fill='#374151')
    s.save('architecture.svg')


def readers():
    s = Svg(1000, 560, 'Readers and data models',
            'Each file format has its own reader. Every reader produces one of four data models: a recording index, an ECG '
            'record, DICOM objects, or the canonical views of a connected database; the viewers and Medorama use only the models.')
    s.text(24, 34, 'Readers: one per format, four data models', size=17, weight=700)
    s.text(24, 54, 'Past its reader, a format is like any other: the viewers, cohorts and the privacy gate know only the data models.', size=12, fill='#4B5563')
    for x, label in [(24, 'Formats'), (296, 'Readers'), (626, 'Data model'), (832, 'Used by')]:
        s.text(x, 84, label, size=12, weight=650, fill='#6B7280')
    rows = [
        (['WFDB: records, multi-segment', 'records, folders of records', 'EDF, EDF+, BDF, BDF+', 'licensed formats (BedMaster .Stp)'],
         ['built in, in Rust, C++ and Python', '(byte-identical; WFDB through wfdb-x)', 'add-ons: programs in any language', 'that speak the adapter protocol'],
         'Recording index', ['channels, pieces, envelopes,', 'events (index-format.md)'],
         ['VitalStream;', 'Medorama (heart', 'rate, cohorts)']),
        (['WFDB, EDF, GE MUSE XML,', 'DICOM ECG, the JSON form;', 'PDF and pictures (as they are)'],
         ['the ECG viewer’s readers: one', 'per format, in one registry', '(JavaScript); muse-py (Python)'],
         'ECG record', ['leads in mV, rate, the', 'report; JSON form'],
         ['ECG viewer']),
        (['DICOM: echo and angiography', 'loops, CT, MR, X-ray, SR;', 'pictures, video, PDF'],
         ['dicom.js and jpeg.js (the', 'browser), dicom-py (Python),', 'dicom.rs (the privacy gate)'],
         'DICOM objects', ['frames, measurements,', 'reports'],
         ['PACS viewer']),
        (['connected databases: OMOP,', 'any SQL database, db-builder', 'SQLite (built from tables and', 'exports: GE MUSE, CLIF)'],
         ['sources/: OMOP views built in,', 'views written by the site, or', 'db-builder’s tables (read only);', 'db-builder reads the tables'],
         'Database views', ['persons, visits, entries,', 'recordings (canonical', 'columns)'],
         ['patient pages, cohorts,', 'Databases tab']),
    ]
    y = 96
    for fmts, rdrs, model, mlines, users in rows:
        h = 24 + 16.5 * max(len(fmts), len(rdrs), 3) + 8
        s.box(24, y, 240, h, 'outside', '', [])
        for i, line in enumerate(fmts):
            s.text(36, y + 22 + 16.5 * i, line, size=11.5, fill='#374151')
        s.box(296, y, 300, h, 'medorama', '', [])
        for i, line in enumerate(rdrs):
            s.text(308, y + 22 + 16.5 * i, line, size=11.5, fill='#374151')
        s.box(626, y, 176, h, 'viewer', model, mlines, title_size=12.5)
        s.box(832, y, 144, h, 'person', '', [])
        for i, line in enumerate(users):
            s.text(844, y + 22 + 16.5 * i, line, size=11.5, fill='#374151')
        mid = y + h / 2
        s.arrow([(264, mid), (296, mid)])
        s.arrow([(596, mid), (626, mid)])
        s.arrow([(802, mid), (832, mid)])
        y += h + 14
    s.text(24, y + 16, 'A new format is one new reader. A reader written in another language plugs in through the adapter protocol (recordings),', size=11.5, fill='#374151')
    s.text(24, y + 34, 'the ECG JSON form, DICOM, or a table or SQL views with the canonical columns (docs/readers.md).', size=11.5, fill='#374151')
    s.save('readers.svg')


def sign_in():
    s = Svg(1000, 440, 'Sign-in: one seam',
            'Users sign in either with Medorama\'s own accounts and passwords, or through the institution\'s authenticating '
            'proxy, which names the user in a header. Both end in one function, signin.rs identify, which gives every handler the '
            'signed-in user; roles, the PHI permission and Privacy Mode are decided in Medorama either way.')
    s.text(24, 34, 'Sign-in: one seam', size=17, weight=700)
    s.text(24, 54, 'How a user signs in is decided in one module; who may see what is decided by Medorama, the same for every method.', size=12, fill='#4B5563')
    s.box(24, 140, 170, 92, 'outside', 'Web browser', ['the Medorama page'])
    s.box(236, 80, 330, 98, 'medorama', 'sign_in = "password"  (the default)', ['POST login: name and password', '(Argon2id, throttled, audited);', 'a session cookie (HttpOnly, SameSite)'])
    s.box(236, 196, 330, 128, 'outside', 'sign_in = "proxy": the institution’s sign-in', ['a reverse proxy in front of Medorama signs the', 'user in: SAML (Shibboleth), OpenID Connect', '(Azure AD, Okta, Google), Kerberos, MFA, VPN;', 'it sets X-Remote-User to the user’s name'], dashed=True)
    s.box(608, 140, 170, 92, 'medorama', 'signin.rs', ['identify(): the one', 'function that knows', 'how users sign in'])
    s.box(820, 120, 156, 132, 'gate', 'Every handler', ['gets Auth(User);', 'roles, PHI', 'permission and', 'Privacy Mode', 'decided by Medorama'])
    s.arrow([(194, 170), (236, 130)])
    s.arrow([(194, 202), (236, 260)])
    s.arrow([(566, 130), (608, 170)])
    s.arrow([(566, 260), (608, 202)])
    s.text(618, 256, 'its header is believed only', size=11, fill='#4B5563')
    s.text(618, 270, 'from proxy_addresses', size=11, fill='#4B5563')
    s.arrow([(778, 186), (820, 186)])
    s.box(24, 346, 952, 74, 'store', 'Adapting it', ['Most institutions need no code: put their sign-in proxy in front and set sign_in = "proxy" (docs/sign-in.md). Another method is a new',
                                                     'variant of signin.rs Method and its arm in identify(); no other file changes. Accounts, roles and the PHI permission stay in Medorama.'])
    s.save('sign-in.svg')



if __name__ == '__main__':
    architecture()
    readers()
    sign_in()
    system_context()
    components()
    privacy_gate()
    data_in()
    sources()
