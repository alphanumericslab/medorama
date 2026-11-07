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
    s = Svg(1000, 610, 'Medorama: system context and trust boundaries',
            'People use a browser; the browser reaches the Medorama server over TLS or an SSH tunnel; the server reads the '
            'data sources, keeps the catalog database, runs the VitalStream viewer and plug-ins on the same host.')
    s.text(24, 34, 'System context and trust boundaries', size=17, weight=700)
    s.text(24, 54, 'Every request crosses one boundary into the server, where sign-in, roles and the privacy gate decide what leaves.', size=12, fill='#4B5563')
    # people
    for i, (who, what) in enumerate([('Researcher', 'cohorts, data, viewers'), ('Clinician', 'a patient’s journey'), ('Data manager (admin)', 'imports, users, review')]):
        s.box(24, 90 + i * 74, 170, 58, 'person', who, [what])
    # browser
    s.zone(214, 76, 220, 240, 'User’s computer (untrusted)')
    s.box(228, 104, 192, 190, 'outside', 'Web browser', ['Medorama page', 'ECG, PACS, VitalStream', 'viewers (static pages)', '', 'stores nothing; shows', 'what the server sends'])
    for i in range(3):
        s.arrow([(194, 119 + i * 74), (228, 119 + i * 74 + (0 if i == 1 else (35 if i == 0 else -35)) * 0)])
    # network
    s.arrow([(420, 200), (548, 200)], both=True)
    for i, line in enumerate(['HTTPS through', 'a reverse proxy,', 'or an SSH tunnel']):
        s.text(484, 166 + 14 * i, line, size=11, fill='#4B5563', anchor='middle')
    # host
    s.zone(540, 76, 440, 496, 'Server host (trusted; the catalog’s OS account)')
    s.box(556, 104, 408, 150, 'medorama', 'Medorama server  (127.0.0.1:8090 by default)',
          ['sign-in (Argon2id), sessions, roles, PHI permission', 'API, patient pages, cohorts, databases, imports', 'privacy gate: Privacy Mode answers, files, exports', 'audit log, job queue and workers'])
    s.box(556, 270, 196, 110, 'viewer', 'VitalStream viewer', ['127.0.0.1, private port,', 'reached only through', 'Medorama’s gateway;', 'add-ons read other formats'])
    s.box(768, 270, 196, 110, 'medorama', 'Plug-in actions', ['separate processes', 'own process group,', 'timeout; get pseudonyms', 'and file paths only'])
    s.arrow([(654, 254), (654, 270)])
    s.arrow([(866, 254), (866, 270)])
    s.box(556, 398, 196, 162, 'phi', 'Catalog database', ['SQLite (owner-only, optional', 'SQLCipher), or PostgreSQL', '(TLS, verified)', 'identifiers (PHI) in their', 'own tables; pseudonyms;', 'users, jobs, audit log'])
    s.box(768, 398, 196, 74, 'store', 'Index and results folders', ['derived data, pseudonymous'])
    s.box(768, 486, 196, 74, 'outside', 'Data sources (read only)', ['exports, recordings, ECGs,', 'DICOM; connected databases'], dashed=True)
    # only the server writes the database; the plug-ins write their results
    s.arrow([(556, 236), (547, 236), (547, 470), (556, 470)])
    s.arrow([(866, 380), (866, 398)])
    s.text(760, 592, 'The server, the viewer and the plug-ins read the data sources; nothing writes to them.', size=11, fill='#4B5563', anchor='middle')
    s.text(24, 330, 'Inputs', size=13, weight=650)
    for i, line in enumerate([
        'Databases (CSV, Excel) imported by an admin,',
        'or connected and read in place (OMOP, SQL).',
        'Files enter only with the rows that name their',
        'patient, read where they are; never written.',
    ]):
        s.text(24, 352 + 17 * i, line, size=11.5, fill='#374151')
    s.text(24, 450, 'Outputs to people', size=13, weight=650)
    for i, line in enumerate([
        'Identified: users with the PHI permission,',
        'Privacy Mode off.  Privacy Mode (default for',
        'everyone but admins): pseudonyms, placeholders,',
        'dates moved back; files de-identified or marked.',
    ]):
        s.text(24, 472 + 17 * i, line, size=11.5, fill='#374151')
    s.save('system-context.svg')


def components():
    s = Svg(1000, 640, 'Medorama server: components and request path',
            'Requests pass the security headers and request check, the privacy layer (session lookup, Privacy Mode decision, '
            'the gate on the way out), then the handlers, which use the core modules over the catalog database.')
    s.text(24, 34, 'Inside the server: one request’s path', size=17, weight=700)
    s.text(24, 54, 'Layers are axum middleware; the privacy layer wraps every route, so every answer passes it on the way out.', size=12, fill='#4B5563')
    s.box(24, 84, 952, 56, 'outside', 'guard  (server.rs)', ['security headers on every answer (CSP, frame rules, nosniff, no-referrer, no-store on the API); X-Medorama: 1 required on every changing API request'])
    s.box(24, 152, 952, 72, 'gate', 'privacy layer  (server.rs privacy, gate.rs)', ['in: finds the session’s user once; Privacy Mode unless the user has the PHI permission and turned it off (POST privacy)',
                                                                                      'out, in Privacy Mode: JSON through the gate; files only if the gate cleared them; anything else refused (fail closed)'])
    cols = [
        ('API  /api/v1', ['patients, report, detail', 'cohorts, databases', 'imports, review (admins)', 'users, audit (admins)', 'files: patients/{id}/file/…']),
        ('Gateway  /viewer/', ['allow-list of viewer API v1', 'per-user browse and open', 'rules; Privacy Mode: tokens,', 'times translated, no', 'folders']),
        ('Pages  /, /viewers/', ['the Medorama page and the', 'ECG and PACS viewers:', 'static files, no data,', 'embedded in the binary']),
        ('Command line  medorama', ['init, import, serve, run,', 'work, users, addons;', 'the local account is the', 'catalog’s owner']),
    ]
    for i, (t, lines) in enumerate(cols):
        s.box(24 + i * 240, 240, 232, 126, 'medorama', t, lines)
    mods = [
        ('gate.rs', ['dictionary of identifiers', 'placeholders, file tokens', 'table cells, notes']),
        ('deid.rs', ['dates moved back', '(per patient), ages 90+', 'WFDB, EDF headers; MUSE XML']),
        ('dicom.rs', ['DICOM headers (PS3.15', 'Basic Profile), pixels', 'copied byte for byte']),
        ('pseudonyms.rs', ['MO-XXXX-XXXX, random,', 'stored, check character,', 'aliases on merges']),
        ('patient/, cohorts/', ['the patient page, search,', 'cohort definitions,', 'exports']),
        ('tables.rs, imports.rs', ['databases in, columns', 'kept as imported,', 'identity, removal']),
        ('users.rs', ['Argon2id, sessions', '(SHA-256 of a random', 'token), roles, PHI']),
        ('jobs.rs, actions.rs', ['queue, workers, indexing,', 'plug-ins as processes', '(pseudonyms only)']),
    ]
    for i, (t, lines) in enumerate(mods):
        x = 24 + (i % 4) * 240
        y = 388 + (i // 4) * 104
        s.box(x, y, 232, 92, 'gate' if t in ('gate.rs', 'deid.rs', 'dicom.rs', 'pseudonyms.rs') else 'medorama', t, lines)
    s.box(24, 600 - 4, 952, 30, 'phi', 'catalog database  (db.rs, sql/: SQLite or PostgreSQL, migrations, owner-only files, optional SQLCipher; identifiers only in their tables)', title_size=12)
    for i in range(4):
        s.arrow([(140 + i * 240, 224), (140 + i * 240, 240)])
    s.save('components.svg')


def privacy_gate():
    s = Svg(1000, 552, 'The privacy gate: how an answer leaves in Privacy Mode',
            'The session decides the mode; in Privacy Mode JSON answers have identifiers replaced, dates moved back and '
            'patient names removed from text; files are de-identified by kind, passed with a warning, or refused.')
    s.text(24, 34, 'The privacy gate: what leaves in Privacy Mode', size=17, weight=700)
    s.text(24, 54, 'The catalog keeps every identifier as imported; the gate decides at the moment of sending. Nothing identifying reaches the browser.', size=12, fill='#4B5563')
    s.box(24, 84, 200, 98, 'outside', 'Request', ['the signed-in user (signin.rs)', '+ Privacy Mode choice', '(set by POST privacy,', 'audited)'])
    s.box(260, 84, 236, 98, 'gate', 'Mode', ['no PHI permission: always on', 'PHI permission: on by default', '(off by default for admins)', 'answered as without PHI'])
    s.box(532, 84, 200, 98, 'medorama', 'Handler', ['reads the catalog;', 'files named by tokens;', 'searches of identifiers', 'refused'])
    s.box(768, 84, 208, 98, 'outside', 'Browser', ['pseudonyms,', '[Patient Name], [MRN] …', 'dates moved back'])
    s.arrow([(224, 133), (260, 133)])
    s.arrow([(496, 133), (532, 133)])
    s.text(744, 214, 'the answer, on its way out', size=12, weight=600, fill='#5C3D00', anchor='middle')
    s.box(24, 230, 300, 186, 'gate', 'JSON answers', ['1  identifiers replaced (the dictionary):', '    patient IDs → MO- pseudonyms', '    encounter IDs → MO-E- pseudonyms', '    MRNs, CSNs → [MRN], [CSN]', '    recordings’ paths → mo-rec tokens', '2  dates and times moved back (per patient)', '3  ages above 89 → 90+; staff → [Staff Name]', '4  the patient’s own names in text → placeholder'])
    s.box(348, 230, 300, 186, 'gate', 'Files, by kind', ['WFDB header: identifying comments out,', '    dates moved, file names → tokens', 'GE MUSE XML: demographics, staff,', '    order out; dates moved', 'DICOM: header rewritten (PS3.15 Basic', '    Profile), pixels as they are', 'EDF, BDF: header de-identified,', '    annotation texts out', 'WFDB samples: numbers only, as they are'])
    s.box(672, 230, 304, 186, 'gate', 'Passed with a warning, or refused', ['pictures, videos, PDF: as they are,', '    marked x-medorama-may-contain-phi', '    (the page shows an amber warning triangle)', 'DICOM: marked too (burned-in text)', 'any other kind of file: refused', 'notes’ text: through the gate, marked', '    (free text may still identify)'])
    s.box(24, 436, 952, 100, 'gate', 'Last layer: fail closed', ['every answer of a Privacy Mode session passes server.rs privacy: JSON through the gate; a file only if the gate cleared it;',
                                                               'any other answer carrying data is refused and logged. A new endpoint that forgets the gate cannot leak: its answer is refused.',
                                                               'Proof: tests/privacy.rs seeds identifiers in every format and checks no byte of any page, file or export holds them.'])
    s.arrow([(614, 182), (614, 198), (174, 198), (174, 230)], gate=True)
    s.arrow([(614, 198), (498, 198), (498, 230)], gate=True)
    s.arrow([(614, 198), (824, 198), (824, 230)], gate=True)
    s.save('privacy-gate.svg')


def import_flow():
    s = Svg(1000, 400, 'Importing a database',
            'A table is inspected, its columns mapped (their names kept), its rows matched to patients and encounters, '
            'their files linked where they are; unmatched rows wait for review; recordings are indexed by workers.')
    s.text(24, 34, 'Importing a database: the only way in', size=17, weight=700)
    s.text(24, 54, 'A file always comes with the row that names its patient. Nothing is written where the data lie.', size=12, fill='#4B5563')
    steps = [
        ('outside', 'Table', ['CSV, or a spreadsheet', '(Excel, OpenDocument):', 'an export of a source', '(EHR, ECG, PACS, …)']),
        ('medorama', 'Inspect', ['columns recognised', '(patient ID, MRN,', 'times, values, file);', 'names kept as imported']),
        ('medorama', 'Match', ['rows to patients and', 'encounters (ID, MRN,', 'encounter, CSN);', 'duplicates to review']),
        ('phi', 'Catalog', ['patients, identifiers', '(PHI), encounters,', 'items, the rows as', 'imported, provenance']),
        ('medorama', 'Index', ['workers index linked', 'recordings, derive', 'series (Holter heart', 'rate); pseudonyms']),
    ]
    for i, (k, t, lines) in enumerate(steps):
        s.box(24 + i * 194, 84, 176, 112, k, t, lines)
        if i:
            s.arrow([(24 + i * 194 - 18, 140), (24 + i * 194, 140)])
    s.box(24 + 2 * 194, 230, 176, 112, 'store', 'Review (admins)', ['rows without a patient', 'wait; possible', 'duplicates merged', 'or kept apart'])
    s.box(24 + 1 * 194, 230, 176, 112, 'outside', 'Files', ['linked where they are,', 'inside data_roots', '(read only)'], dashed=True)
    s.arrow([(24 + 2 * 194 + 88, 196), (24 + 2 * 194 + 88, 230)])
    s.arrow([(24 + 1 * 194 + 88, 196), (24 + 1 * 194 + 88, 230)])
    s.box(24 + 3 * 194, 230, 370, 112, 'store', 'Removal', ['removing a database takes out everything it added,', 'everywhere: its items, values and notes, the', 'recordings and identifiers that only it brought'])
    s.text(24, 380, 'Requirements for each table: docs/importing.md. Identifiers stay in the catalog’s identifier tables; every import and removal is audited.', size=11.5, fill='#374151')
    s.save('import-flow.svg')


def sources():
    s = Svg(1000, 600, 'Medorama: sources and the control store',
            'The Medorama servers keep their own data in the control store (SQLite, or PostgreSQL for several servers), read the '
            'catalog of imported databases and connected databases (OMOP, mapped SQL) in place, link each source\'s persons to one '
            'patient in the registry, and send every answer through the privacy gate.')
    s.text(24, 34, 'Sources and the control store', size=17, weight=700)
    s.text(24, 54, 'One patient, one pseudonym, across every source; every answer through the same gate; connected databases only read.', size=12, fill='#4B5563')
    s.box(24, 84, 952, 46, 'outside', 'Browser, command line, plug-ins', ['HTTPS to any server: sessions, cohorts and the job queue are in the control store'])
    s.zone(24, 146, 952, 238, 'Medorama server  (one with SQLite; several identical ones with PostgreSQL)')
    s.box(40, 174, 920, 40, 'gate', 'privacy gate  (gate.rs): every answer and file leaves through it; only canonical fields reach it', title_size=12.5)
    s.box(40, 226, 300, 146, 'medorama', 'Features', ['cohorts (criteria across sources)', 'the patient page (every source’s', '    entries in the same columns)', 'databases, exports, jobs, gateway'])
    s.box(356, 226, 300, 146, 'medorama', 'Query layer', ['planner (cohorts/): each criterion', '    to each source that holds such data,', '    its filters run there', 'registry (sources/link.rs): persons', '    linked to patients, by ID, then MRN'])
    s.box(672, 226, 288, 146, 'medorama', 'Source layer', ['the catalog: imported databases', 'connected databases (sources/):', '    three views of canonical columns', '    OMOP built in; any SQL mapped', 'sql/: SQLite or PostgreSQL, one API'])
    s.box(24, 408, 300, 168, 'phi', 'Control store', ['SQLite (one server) or PostgreSQL', 'users, sessions, audit, settings', 'the catalog’s imported databases', 'patients, identifiers (PHI),', '    pseudonyms, the registry', 'cohorts and their sources’ versions', 'the job queue (advisory locks)'])
    s.box(350, 408, 300, 168, 'outside', 'Connected databases (read only)', ['OMOP CDM 5.3, 5.4 · mapped SQL', 'PostgreSQL: on-site, Amazon RDS', '    and Aurora, Azure; SQLite files', 'read-only sessions; TLS required', '    to another host, verified', 'per-query time limit, row limit', 'password in a file, never stored'], dashed=True)
    s.box(676, 408, 300, 168, 'outside', 'Files (read only)', ['recordings (WFDB; others by add-ons)', 'ECGs (WFDB, MUSE XML, DICOM)', 'DICOM studies, pictures, PDFs', 'inside data_roots, linked by the', '    rows that name their patient'], dashed=True)
    s.arrow([(174, 384), (174, 408)], both=True)
    s.text(184, 400, 'read, write', size=11, fill='#4B5563')
    s.arrow([(500, 384), (500, 408)])
    s.text(510, 400, 'read only', size=11, fill='#4B5563')
    s.arrow([(826, 384), (826, 408)])
    s.text(836, 400, 'read only', size=11, fill='#4B5563')
    s.arrow([(500, 130), (500, 146)], both=True)
    s.save('sources.svg')


def architecture():
    s = Svg(1000, 716, 'Medorama: architecture at a glance',
            'People use a browser; requests pass the institution\'s front door (TLS, optionally its single sign-on), then '
            'Medorama\'s sign-in, roles and handlers; every answer leaves through the privacy gate. Readers turn each file '
            'format into one of four data models. Identifiers are kept only in the catalog database; data sources are only read.')
    s.text(24, 34, 'Medorama at a glance', size=17, weight=700)
    s.text(24, 54, 'One server, one way in, one way out: sign-in decides who is asking, the privacy gate decides what leaves.', size=12, fill='#4B5563')
    s.box(24, 80, 220, 76, 'person', 'People', ['researchers, clinicians,', 'data managers, admins'])
    s.box(276, 80, 220, 76, 'outside', 'Web browser', ['the Medorama page and viewers:', 'static pages; stores no data'])
    s.box(528, 80, 448, 76, 'outside', 'The institution’s front door', ['HTTPS (a reverse proxy), and where required its sign-in:', 'single sign-on, MFA, VPN — the part each institution adapts'], dashed=True)
    s.arrow([(244, 118), (276, 118)])
    s.arrow([(496, 118), (528, 118)], both=True)
    s.zone(24, 178, 952, 346, 'Medorama server')
    s.box(40, 206, 284, 80, 'medorama', '1  Sign-in  (signin.rs)', ['who is asking: a password, or the', 'name the institution’s proxy sends'])
    s.box(346, 206, 284, 80, 'medorama', '2  Roles and the PHI permission', ['what that user may do and see', '(users.rs): admin, analyst, viewer'])
    s.box(652, 206, 308, 80, 'medorama', '3  Handlers', ['the API, the viewer gateway, the pages;', 'imports, cohorts, patient pages, jobs'])
    s.arrow([(752, 156), (752, 168), (280, 168), (280, 206)])
    s.arrow([(324, 246), (346, 246)])
    s.arrow([(630, 246), (652, 246)])
    s.box(40, 302, 920, 64, 'gate', '4  The privacy gate  (gate.rs, deid.rs, dicom.rs)', ['every answer, file and export of a session in Privacy Mode leaves through it: identifiers replaced, dates moved back,',
                                                                                         'files de-identified at the source; an answer it did not clear is refused (fail closed)'])
    s.arrow([(806, 286), (806, 302)], gate=True)
    s.box(40, 382, 440, 126, 'medorama', 'Readers: one per file format', ['WFDB, EDF, ECG and DICOM files, tables (CSV, Excel, Parquet),', 'known exports (GE MUSE, CLIF), connected databases', '(OMOP, SQL views); licensed formats through add-ons', 'Past the reader, every format is one of four data models:', 'recordings, ECGs, images, tables'])
    s.box(496, 382, 220, 126, 'viewer', 'VitalStream viewer', ['waveforms; a private port', 'on 127.0.0.1, reached only', 'through Medorama’s', 'gateway'])
    s.box(732, 382, 228, 126, 'medorama', 'Jobs, plug-ins, add-ons', ['indexing and actions;', 'plug-ins and adapters run as', 'separate programs and', 'get pseudonyms, not PHI'])
    s.box(24, 548, 300, 104, 'phi', 'Catalog database', ['SQLite or PostgreSQL: users, audit,', 'patients, pseudonyms; the only', 'place identifiers (PHI) are kept'])
    s.box(350, 548, 300, 104, 'store', 'Indexes and results', ['what Medorama derives', '(waveform indexes, action', 'results): pseudonymous'])
    s.box(676, 548, 300, 104, 'outside', 'Data sources  (read only)', ['files where they lie, inside the', 'allowed folders; connected', 'databases, through read-only views'], dashed=True)
    s.arrow([(174, 524), (174, 548)], both=True)
    s.text(184, 540, 'read, write', size=11, fill='#4B5563')
    s.arrow([(500, 524), (500, 548)])
    s.text(510, 540, 'write', size=11, fill='#4B5563')
    s.arrow([(826, 548), (826, 524)])
    s.text(836, 540, 'read only', size=11, fill='#4B5563')
    s.text(24, 680, 'Numbers: a request’s path. Dashed: outside Medorama. The server runs as one operating-system account and listens on 127.0.0.1 by default.', size=11.5, fill='#374151')
    s.text(24, 698, 'An institution adapts its front door, and at most one module of Medorama (signin.rs).', size=11.5, fill='#374151')
    s.save('architecture.svg')


def readers():
    s = Svg(1000, 560, 'Readers and data models',
            'Each file format has its own reader. Every reader produces one of four data models: a recording index, an ECG '
            'record, DICOM objects, or catalog tables; the viewers and Medorama use only the models.')
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
        (['CSV, Excel, Parquet tables;', 'known exports (GE MUSE, CLIF);', 'OMOP and other SQL databases'],
         ['tables.rs; formats/ (converters,', 'or a TOML description: no code);', 'sources/ (read-only SQL views)'],
         'Catalog tables', ['patients, encounters,', 'entries (canonical columns)'],
         ['patient pages, cohorts,', 'Databases']),
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
    s.text(24, y + 34, 'the ECG JSON form, DICOM, or a table with the canonical columns (docs/readers.md).', size=11.5, fill='#374151')
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
    import_flow()
    sources()
