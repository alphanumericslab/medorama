# Medorama: HTTP API

This page describes the JSON API that the Medorama page and every tool use, under `api/v1/`. It
is written for developers who script Medorama or integrate it with other systems. The
machine-readable description is [`medorama/openapi.yaml`](../medorama/openapi.yaml) (OpenAPI 3.1);
a test keeps it equal to the server's routes. What each tab of the page does with these routes is
described in [user-guide.md](user-guide.md).

## Conventions

- **Sign-in.** How users sign in depends on the `sign_in` setting
  ([sign-in.md](sign-in.md)), which `GET sign-in` reports before anyone is signed in:
  - With `password` (the default), `POST login` checks a name and a password and sets the
    session cookie (HttpOnly, SameSite=Strict, and `Secure` with `secure_cookies`). Every other
    request, except `GET sign-in`, needs that cookie.
  - With `proxy`, the institution's authenticating reverse proxy signs the user in and names them
    in a header (`proxy_user_header`, by default `X-Remote-User`), which Medorama believes only
    from the addresses in `proxy_addresses`. `POST login` and `POST me/password` are then refused
    (400). A name the proxy sends that has no enabled account in Medorama is refused with 403 and
    a reason the page shows.
- **Changes.** Every request other than GET, `POST login` included, needs the header
  `X-Medorama: 1`. The page sends it, and a cross-site form cannot; without it, the answer is 403.
- **Answers** are JSON, sent with `Cache-Control: no-store`, with status 200. Errors are
  `{"detail": "…"}`, with these statuses:

  | Status | When |
  |---|---|
  | 400 | The request is wrong (a missing or invalid value, a search that cannot be parsed, a database that cannot be connected), or asks for a password sign-in or change when users sign in through a proxy. |
  | 401 | Not signed in, or the session ended. |
  | 403 | The role, the PHI permission or a grant does not allow it; the `X-Medorama` header is missing; Privacy Mode does not let the answer out; the proxy names a user without an account. |
  | 404 | No such thing, or one the user is not granted (a database, a recording, a cohort). |
  | 413 | The body is larger than 2 MB. |
  | 429 | Too many failed sign-ins: the user name is locked for a minute. |
  | 500 | A fault of the server (also written to its standard error). |
  | 503 | The control store's one writer is busy (SQLite, while a database is refreshed), with `Retry-After: 5`; or `POST viewer/start` cannot start the viewer. |

  A body that is not JSON of the expected shape is refused before the route runs, with a
  plain-text error: 400 (not JSON), 415 (no `Content-Type: application/json`) or 422 (a field
  missing, of the wrong type, or unknown to a route that refuses unknown fields).
- **Privacy Mode** (chosen with `POST privacy` and kept by the server for the session; a new sign-in starts in the user's default mode) applies to every answer
  of the session: identifiers become placeholders and pseudonyms, dates move back, and files pass
  the privacy gate ([user-guide.md, Privacy Mode](user-guide.md#privacy-mode)). A file that the
  gate sends as it is (a picture, a video, a PDF) carries the header
  `x-medorama-may-contain-phi: 1`.
- **Roles and grants.** The table below says who may call each route. A role is not enough for
  a database's data: each database and each cohort is used only by the users granted it
  ([user-guide.md, Terms](user-guide.md#terms-used-in-this-guide)). On a database, `search`
  lets criteria find its patients; `view` also shows its data on patient pages, its recordings
  and the viewer; `phi` also sends its identifiers, to a user with the PHI permission outside
  Privacy Mode; `manage` also lets one check, refresh, grant and remove it, and see its URL,
  settings, status error and identity conflicts. On a cohort: `view`, `edit`, `manage`. Each
  permission includes the weaker ones. The admin who connects a database, or the user who makes a
  cohort, gets every permission on it; other admins are not granted a database by being admins,
  but may grant it to themselves (audited as `grant_self`). A database or recording the user is
  not granted answers 404, as if it did not exist.

## Routes

Paths are relative to `api/v1/`. In the Role column, "any" means any signed-in user, "analyst"
an analyst or an administrator, and "admin" an administrator; a grant named there is needed on
the database or cohort the route names.

| Method, path | Role | Does |
|---|---|---|
| `GET sign-in` | (public) | how users sign in: `{method, logout_url}`, with `method` `password` or `proxy`, and `logout_url` the proxy's sign-out address (`proxy_logout_url`; empty with `password`) |
| `POST login` `{username, password}` | (public) | sign in (sets the cookie); 400 when users sign in through a proxy |
| `POST logout` `{everywhere}` | | sign out: `{logout_url}`. With `password`, ends this session, or with `everywhere` every session of the user (audited); with `proxy`, `logout_url` is where the page sends the browser to end the proxy's session. The Privacy Mode cookie is cleared either way |
| `GET me`, `POST me/password` `{old, new}` | any | who am I; change my password (400 when users sign in through a proxy) |
| `GET config` | any | `version`, `demo` (a demo installation: invented patients only; [design/demo.md](design/demo.md)), `timezone`, `sign_in` (`password` or `proxy`), `viewer_url` (a viewer running elsewhere, opened in a new browser tab; empty when Medorama runs its own), `auto_index`, `workers`, `user`, `viewer` (its status), `deid` (Privacy Mode's settings), `display` (date and time formats); `workspace_dir` for admins (else null); `db_builder` for admins (else null): `mode` (`off`, `run`: Medorama runs it, `link`: `db_builder_url`), `url` (its page, opened by **Create a database…**), `running`, `reason` (why it does not run) |
| `POST privacy` `{on}` | any | switch the session's Privacy Mode (off: PHI permission only); audited |
| `GET deid`, `PUT admin/deid` `{fields, offset: {years, months, days, hours}, spread_days}` | admin | what Privacy Mode withholds, how dates move (the offset; `spread_days` from 30 to 3650), the choices (`fields`: `dates`, `age_over_89`, `notes`; names, MRNs, dates of birth and account numbers are always withheld and not listed); withholding less (a choice unticked, fewer days, a smaller offset) needs the PHI permission (403); `GET config` gives other users what is withheld, without the offset |
| `PUT admin/display` `{date, clock}` | admin | how the page writes dates and times (`date`: `ymd`, `mdy`, `dmy`, `dmy_dot`; `clock`: `24h`, `12h`) |
| `GET admin/addons` | admin | the installed adapters: `{dir, items: [{name, version, implementation, program, formats}]}` |
| `GET health` | none (no session) | liveness for a service manager or load balancer: `{ok, control_store, background}`; 200 when the control store answers and the background thread (checks, audit sealing, automatic indexing) went round in the last two minutes, else 503. Nothing else: no names, no counts |
| `GET version` | any | `{data}`: the data's version, which moves when a database is connected, refreshed or removed, or patients are linked (open pages redraw when it moves) |
| **Patients** | | |
| `GET patients?q&has&cohort&mark&limit&offset` | any; `view` | the patients of the databases the user may view, latest first: `{total, items, categories}`; `q`: the [search](user-guide.md#searching) (`patient`, `sex`, `has`, `database`; `name`, `first`, `last`, `mrn`, `dob` with the PHI permission, of the databases granted `phi`); `has`: categories, comma-separated (`!icu`: without); `cohort`: its members only; `mark`: each item says whether it is in that cohort. The patients, their identity and their encounters come from what Medorama keeps of each database, so it answers while a database is offline; for the page asked (`limit` up to 1,000), each online database is asked what it holds of those patients: an item's `kinds` (categories of data), `items` (its entries; `null` when no database was read) and `latest`. A `has` filter asks each online database |
| `GET patients/{id}` | any; `view` | a patient's summary, read from each database the user may view: identity (PHI, only from the databases granted `phi`), visits (each with its database), problems, medications, allergies, the databases that hold the patient (`sources`) and those that did not answer (`unavailable`); adds the patient to the history |
| `GET patients/{id}/journey` | any; `view` | the journey: columns, 6-hour cells (entries, worst level), spans (stays, drugs, recordings) |
| `GET patients/{id}/report?q` | any; `view` | the report's entries, one per thing done, by name: `{entries: [{key, time, end, category, title, names, encounter, snippet, attachment}], counts, terms, fuzzy}`; `title` says what was done, never a value or a flag (a panel by its tests, `Vital signs · N readings`, `12-lead ECG`); `names` lists what it holds (a panel's tests, an ECG's measurements), which the journey's columns find it by; an ECG with its measurements is one entry (`ecg:<database>:<token>`); with `q`, the search (counts per category, words highlighted, fuzzy matches) |
| `GET patients/{id}/detail?key` | any; `view` | one entry's detail (panel rows, text, attachment, ICU overview, notes around); `key`: `item:…`, `labs:<patient>:<time>`, `vit:<patient>:<day>`, `ecg:<database>:<token>`, `rec:…`, `live:…`. An ECG's has `ecg`: `measures` (`[{name, value, unit, change, series}]`: `change` this value less the patient's previous ECG's, `null` when that one has none; `series` `[time, value]` over the patient's ECGs, oldest first), `statements` (`[[text, new]]`, `new` when the previous ECG did not say it; empty when Privacy Mode withholds notes' text), `gone` (the previous ECG's statements no longer said) and `previous` (that ECG's time, or `null`) |
| `GET patients/{id}/trends` | any; `view` | every numeric series of the patient |
| `GET patients/{id}/file/{item}` | any; `view` | an entry's attached file, or a recording's (`item`: a recording's number); for a folder (a DICOM series), its files: `{files: [names]}`. In Privacy Mode, and for a database the user holds without `phi`, through the gate, names as tokens (`mo-f0003.dcm`). Sent with `Cache-Control: no-store` |
| `GET patients/{id}/file/{item}/{name}` | any; `view` | a file of that attachment by name: the file, the signal files its WFDB header names, a file of its folder; nothing else of the folder it lies in. In Privacy Mode, by token only |
| `GET history`, `DELETE history/{patient}`, `DELETE history` | any | my history: the patients whose page I opened, latest first; forget one; forget all |
| **Cohorts** | | |
| `POST cohorts/search` `{criteria, databases, source, within_source_of, limit, offset}` | any; `search` | the patients meeting the criteria, with why: `{total, items, ids, excluded_total, excluded, searched, left_out}`. `databases`: the databases ticked (empty: every one the user may search); only those granted `search` and online are searched; `searched` names them and `left_out` the others with why (`not ticked`, `offline`) |
| `POST cohorts/parse` `{text}` | any | criteria written as text: `{ok, expr, words}` or `{ok: false, error}`; names resolve against the variables of the databases the user may search |
| `GET cohorts`, `POST cohorts` `{name, description, criteria, databases, source, patients}` | any | the cohorts one may see (own, or granted), each with its `definition`; make one (`databases`: those it searches, empty: every one; `source`: `{op: within \| union \| intersection \| difference, of: [ids]}`; no `patients`: all the definition selects). Its maker gets every permission on it |
| `GET cohorts/{id}`, `PATCH cohorts/{id}` `{name, description, criteria, databases, add, add_search, remove, shares}`, `DELETE cohorts/{id}` | `view`; `edit` (`shares`: `manage`); `manage` | a cohort, with the databases it searches and their versions when its members were found (`source_versions`); change it (`shares`: the users granted `view`, or `edit` with `can_edit`); delete it: its key is destroyed, then its workspace file; cohorts made from it keep its last members |
| `POST cohorts/{id}/clone` `{name}`, `GET cohorts/{id}/history` | `view` | a copy of one's own; its changes, latest first |
| `GET cohorts/{id}/members?limit&offset`, `POST cohorts/{id}/refresh` `{dry_run, join, leave, remove}` | `view`; `edit` | its members; what its definition selects now (`added`, `removed`; a dry run lists who would join and leave), applied unless `dry_run` |
| `GET cohorts/{id}/export?what&privacy` | `view` | `what`: `members` (CSV, the default), `criteria` (the definition, JSON), `data` (the members' data, CSV), `xlsx`; in Privacy Mode or with `privacy=1`, through the gate |
| `GET cohorts/people` | any | the users a cohort or a database can be granted to (names only) |
| **Databases** | | |
| `GET databases` | any | the databases granted to the user (admins: every one, to grant it), online first: `{items: [{id, name, kind, engine, status, status_error, checked_at, version, recordings, has_recordings, permission, persons, patients, conflicts, variables, created_at, url, settings}]}` (`recordings`: those Medorama has opened or indexed; `has_recordings`: whether it lists recordings). Counts only to those granted; `url` (without a password), `settings` and `status_error` only to who manages it. `status`: `online`, `offline` or `unknown` (not checked yet) |
| `POST databases` `{name, kind, url, password, ca_file, settings}` | admin | connect a database: `kind` `omop`, `views` or `dbb`; `url` `sqlite:/path/file.sqlite` (inside `data_roots`, never inside the workspace) or `postgres://…` (no password in it); `password` kept wrapped by the master key, never written to a file nor sent back; `settings`: `schema`, `vocabulary_schema`, `identifier` (OMOP: `mrn`, `patient_id`, `none`), `issuer`, `mrn_authority`, `timezone`, `files_root`, `timeout_s`, `max_rows`, and for `views` the queries `persons`, `visits`, `entries`, `recordings`, `stays`, `version`, `variables` ([databases.md](databases.md#canonical-views-reference)). Checked first; then its data key and workspace folder are made, its catalog read (names and units, not counted), its persons linked and their encounters kept, and every permission on it granted to the admin; no recording is read: `{id}` |
| `DELETE databases/{id}` | `manage` | remove the database and every trace of it: its data key destroyed (and those of the results derived from it), then its workspace folder, then every cohort row whose provenance includes it, its patient links, the patients no database links any more (with their pseudonyms), its jobs, grants and row. Audited. The database itself is never touched. A removal a crash interrupted finishes when the server or any command starts |
| `POST databases/{id}/check` | `search` | check it now: `{online, status, version, checked_at}`; when its data version changed, its catalog (variables' names and units), persons and encounters are read again |
| `POST databases/{id}/refresh` | `manage` | read its catalog, persons and encounters again now; no recording is read |
| `POST databases/{id}/count` | `manage` | count its variables (**Count now**): how many entries and persons hold each, one grouped query over its entries: `{counted}` (the variables counted). Refused while it is offline. Audited (`count`) |
| `POST databases/{id}/index` | `manage` | read every recording it lists (at most `max_rows`), check their files and queue the new or changed ones for indexing (**Index all recordings**): `{recordings, added, changed, missing, removed, queued}`. Refused while it is offline. Audited (`index_all`) |
| `GET databases/{id}/variables` | `search` | what it holds, kept per data version: `{items: [{category, name, vocabulary, code, unit, entries, persons, numeric}], counted_at}`; `entries` and `persons` are `null` until counted (`counted_at` `null` too), and a count from 1 to 10 is the text `"< 11"` (other counts as text too); kept while it is offline |
| `GET databases/{id}/conflicts` | `manage` | its identity conflicts: persons whose identifiers name two patients, `{items: [{person_key, detail}]}`, by the database's own key (never their identifiers); they are not linked |
| `GET databases/{id}/grants`, `POST databases/{id}/grants` `{user_id, permission}`, `DELETE databases/{id}/grants` `{user_id, permission}` | `manage` (`POST` without `user_id`: an admin) | who may use it: `{items: [{user_id, username, permission}]}`; grant `search`, `view`, `phi` or `manage` (without `user_id`: to the admin asking, audited as `grant_self`); revoke a permission and the stronger ones (one's own `manage` cannot be revoked). Audited. Like `GET databases/{id}/conflicts`, 404 to who may not search the database, whether it exists or not; 403 to who may search it without managing it |
| `GET server-files?dir&kind` | admin | a folder on the server, inside `data_roots`, as the page's **Choose…** dialog shows it: `{dir, parent, items: [{name, path, dir, size}], truncated}`; `kind`: `databases` (default: SQLite files) or `folders`; without `dir`, the data roots; `parent` empty above a root. Refused (403) in Privacy Mode |
| **Recordings** | | |
| `GET recordings?q&limit&offset` | any; `view` | the recordings Medorama keeps of the recording databases the user may view (those opened or indexed), newest first, matching words in their name, format, status or database (de-identified: their token, label, format, status, database, never their file name; sorted by times as shown): `{total, items}`. A recording's number is its database's id × 1,000,000,000 + its number in that database |
| `GET recordings/{id}` | any; `view` | a recording and the results of the actions run on it (each job's `summary`); de-identified, the whole answer moves by its patient's offset, times under a summary's own keys too |
| `POST check` | admin | re-check every kept recording's file: `{checked, missing, returned, changed, index_lost}` |
| **Work** | | |
| `GET actions`, `POST actions/reload` | any, admin | the actions |
| `POST runs` `{actions, selection, params, force, priority}` | analyst; `view` | queue a run on recordings of the databases the user may view. `selection`: `{"recordings": [ids]}`, `{"patients": [ids]}`, `{"cohort": id}` (its members' recordings) or `{"all": true}` |
| `GET jobs?state&run&limit` | any | jobs and counts per state (non-admins: those of the databases granted `view`, the errors of their own runs only); `output_dir` for admins only; in Privacy Mode a failed job's error is `failed` |
| `POST jobs/cancel` `{job}` or `{run}` (or `{}`: all, admin) | analyst | cancel (a viewer: 403) |
| **Users and the audit log** | | |
| `GET users`, `POST users`, `PATCH users/{id}`, `DELETE users/{id}` | admin | users; `POST users` `{username, role, phi, display_name, password}` (with proxy sign-in the password may be left out: the account gets one nobody knows); `PATCH` `{sign_out: true}` ends the user's sessions everywhere (audited); a `password` for a user with the PHI permission needs it (403), ends that user's sessions and is audited `phi_password_reset` |
| `GET audit?limit`, `GET audit/verify` | admin | the audit log; whether its seals hold: `{seals, entries, unsealed, broken, exported_seals}` |
| **The waveform viewer** | | |
| `GET viewer`, `POST viewer/start` | any | the viewer's status `{mode: embedded\|link\|none, flavor, running, reason, url}`; start it now |

The VitalStream viewer itself is served under `/viewer/`, not under `api/v1`, behind Medorama's
sign-in and its gateway ([administration.md, The waveform viewer](administration.md#the-waveform-viewer)). A user who is not an admin
opens there only the recordings of the databases granted to them with `view`. The ECG and PACS
viewers' pages are served under `/viewers/ecg/` and `/viewers/pacs/`. They hold no data: they
read the files above with the user's session.
