# Medorama: HTTP API

This page describes the JSON API that the Medorama page and every tool use, under `api/v1/`. It
is written for developers who script Medorama or integrate it with other systems. The
machine-readable description is [`medorama/openapi.yaml`](../medorama/openapi.yaml) (OpenAPI 3.1);
a test keeps it equal to the server's routes. What each tab of the page does with these routes is
described in [user-guide.md](user-guide.md).

## Conventions

- **Sign-in.** How users sign in depends on the catalog's `sign_in` setting
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
- **Answers** are JSON, sent with `Cache-Control: no-store`: 200, or 202 for an import started in
  the background (`background`: the answer is its task, followed with `GET import-tasks/{id}`).
  Errors are `{"detail": "…"}`, with these statuses:

  | Status | When |
  |---|---|
  | 400 | The request is wrong (a missing or invalid value, a search that cannot be parsed), or asks for a password sign-in or change when users sign in through a proxy. |
  | 401 | Not signed in, or the session ended. |
  | 403 | The role or the PHI permission does not allow it; the `X-Medorama` header is missing; Privacy Mode does not let the answer out; the proxy names a user without an account. |
  | 404 | No such thing. |
  | 413 | The body is too large: 1 GiB for the uploads (`tables/inspect`, `tables`, `tables/files`), 2 MB for any other request. |
  | 429 | Too many failed sign-ins: the user name is locked for a minute. |
  | 500 | A fault of the server (also written to its standard error). |
  | 503 | The catalog's one writer is busy saving an import (SQLite), with `Retry-After: 5`; or `POST viewer/start` cannot start the viewer. |

  A body that is not JSON of the expected shape is refused before the route runs, with a
  plain-text error: 400 (not JSON), 415 (no `Content-Type: application/json`) or 422 (a field
  missing, of the wrong type, or unknown to a route that refuses unknown fields, such as the
  removals).
- **Privacy Mode** (chosen with `POST privacy` and kept by the server for the session; a new sign-in starts in the user's default mode) applies to every answer
  of the session: identifiers become placeholders and pseudonyms, dates move back, and files pass
  the privacy gate ([user-guide.md, Privacy Mode](user-guide.md#privacy-mode)). A file that the
  gate sends as it is (a picture, a video, a PDF) carries the header
  `x-medorama-may-contain-phi: 1`.
- **Roles.** The table below says who may call each route. Identifiers are sent only to users
  with the PHI permission, outside Privacy Mode.

## Routes

Paths are relative to `api/v1/`. In the Role column, "any" means any signed-in user, "analyst"
an analyst or an administrator, and "admin" an administrator.

| Method, path | Role | Does |
|---|---|---|
| `GET sign-in` | (public) | how users sign in: `{method, logout_url}`, with `method` `password` or `proxy`, and `logout_url` the proxy's sign-out address (`proxy_logout_url`; empty with `password`) |
| `POST login` `{username, password}` | (public) | sign in (sets the cookie); 400 when users sign in through a proxy |
| `POST logout` `{everywhere}` | | sign out: `{logout_url}`. With `password`, ends this session, or with `everywhere` every session of the user (audited); with `proxy`, `logout_url` is where the page sends the browser to end the proxy's session. The Privacy Mode cookie is cleared either way |
| `GET me`, `POST me/password` `{old, new}` | any | who am I; change my password (400 when users sign in through a proxy) |
| `GET config` | any | `version`, `timezone`, `sign_in` (`password` or `proxy`), `viewer_url` (a viewer running elsewhere, opened in a new browser tab; empty when Medorama runs its own), `auto_index`, `workers`, `user`, `viewer` (its status), `deid` (Privacy Mode's settings), `display` (date and time formats); `results_dir` and `index_dir` for admins (else null) |
| `POST privacy` `{on}` | any | switch the session's Privacy Mode (off: PHI permission only); audited |
| `GET deid`, `PUT admin/deid` `{fields, offset: {years, months, days, hours}, spread_days}` | admin | what Privacy Mode withholds, how dates move (the offset; `spread_days` from 30 to 3650), the choices; `GET config` gives other users what is withheld, without the offset |
| `PUT admin/display` `{date, clock}` | admin | how the page writes dates and times (`date`: `ymd`, `mdy`, `dmy`, `dmy_dot`; `clock`: `24h`, `12h`) |
| `GET admin/addons` | admin | the installed adapters: `{dir, items: [{name, version, implementation, program, formats}]}` |
| `GET version` | any | `{data}`: the data's version, which moves at every import, removal or change (open pages redraw when it moves) |
| **Patients** | | |
| `GET patients?q&has&cohort&mark&limit&offset` | any | patients, latest encounter first: `{total, items, categories}`; `has`: categories, comma-separated (`!icu`: without); `cohort`: its members only; `mark`: each item says whether it is in that cohort |
| `GET patients/{id}` | any | a patient's summary: identity (PHI), encounters with bed stays, problems, medications, allergies; the connected databases' stays (`visits`) and those that did not answer (`unavailable`) |
| `GET patients/{id}/journey` | any | the journey: columns, 6-hour cells (items, worst level), spans (stays, drugs, recordings) |
| `GET patients/{id}/report?q` | any | the report's entries; with `q`, the search (counts per category, words highlighted, fuzzy matches) |
| `GET patients/{id}/detail?key` | any | one entry's detail (panel rows, text, attachment, ICU overview, notes around) |
| `GET patients/{id}/trends` | any | every numeric series of the patient |
| `GET patients/{id}/file/{item}` | any | an item's attached file; for a folder (a DICOM series), its files: `{files: [names]}`. In Privacy Mode through the gate, names as tokens (`mo-f0003.dcm`) |
| `GET patients/{id}/file/{item}/{name}` | any | a file of that attachment by name: the file, the signal files its WFDB header names, a file of its folder; nothing else of the folder it lies in. In Privacy Mode, by token only |
| **Encounters and recordings** | | |
| `GET encounters?q&unit&from&to&status&limit&offset` | any | search ([syntax](user-guide.md#searching)): `{total, items}` |
| `GET encounters/{id}` | any | an encounter with bed stays, recordings (status, link, results), item counts, the patient's other encounters; adds it to the history |
| `GET encounters/{id}/ehr?domain&patient_level&limit&offset` | any | items in time order (`patient_level`: the patient's items outside every encounter) |
| `POST encounters/remove` `{ids, dry_run}`, `POST encounters/remove-matching` `{search, dry_run}` | admin | remove encounters (with their bed stays and items; recordings stay): `{removed, report}`, and the report (`{dry_run, encounters, patients, bed_stays, ehr_items, …}`); `search` as `GET encounters` takes it; with `dry_run`, what would go, and nothing changes. Other fields (`purge`, `q`, `unlinked`) are refused (422) |
| `GET recordings?q&unlinked&limit&offset`, `GET recordings/{id}` | any | recordings |
| `POST recordings/remove` `{ids, purge, dry_run}`, `POST recordings/remove-matching` `{q, unlinked, purge, dry_run}` | admin | remove recordings (their files untouched): `{dry_run, removed}`, `{dry_run, recordings}`; `q` and `unlinked` as `GET recordings` takes them; `purge` also deletes their results and indexes (inside `results_dir` and `index_dir`); with `dry_run`, how many would go, and nothing changes. Other fields are refused (422) |
| `POST recordings/{id}/link` `{encounter}` | admin | link manually (`null`: forget the manual links) |
| `POST recordings/{id}/decision` `{encounter_id, decision}` | admin | `link`, `never`, or `null` (forget the decision) |
| `POST check` | admin | re-check the recordings |
| **Review** | | |
| `GET review` | admin | held-back rows, waiting items; for PHI users also possible duplicates and recordings whose files disagree |
| `POST review/held/{id}` `{action}` | admin | `apply` (with the fix the problem calls for) or `dismiss` |
| `POST review/merge` `{a, b}`, `POST review/distinct` `{a, b}` | admin + PHI | merge patient `a` into `b`; mark them as different people (403 without the PHI permission, or in Privacy Mode) |
| **Databases and imports** | | |
| `GET databases` | any | the databases in use: rows, patients reached, categories, held-back rows, waiting items; a connected database's `connection` |
| `GET databases/{id}/rows?q&f<pos>&sort&desc&limit&offset` | any | a database's rows as imported (one page): columns (`header`, `maps_to`), the columns not shown, each row's cells, status, patient, entry and file link; `f<pos>`: a column's filter |
| `GET databases/{id}/export?privacy` | admin | the database as CSV, as imported; in Privacy Mode or with `privacy=1` (or `deid=1`, its earlier name), through the gate |
| `GET formats` | admin | the import formats (built in and the site's), the files that could not be read, the categories |
| `POST tables/inspect?name&format` (body: the file) | admin | headers, the mapping and category they suggest, the format it is (converted), the catalog's columns by group; the first rows for PHI users |
| `POST tables?name&batch&source&category&mapping&format&dry_run` (body: the file) | admin | import a database (one batch); `mapping` is JSON or TOML (empty: by the headers); `format`: `auto` (default), `none` or a format's key |
| `POST tables/files?batch&source&category&mapping&format&dry_run&background` (multipart: each `file`) | admin | one database from one or several files with the same columns; with `background`, the answer (202) is the import's task |
| `POST tables/clif` `{dir, source, dry_run, background}` | admin | a CLIF folder on the server (inside `data_roots`): each table a database |
| `GET server-files?dir&kind` | admin | a folder on the server, inside `data_roots`, as the page's **Choose on the server** dialog shows it: `{dir, parent, items: [{name, path, dir, size}], truncated}`; `kind`: `tables` (default: CSV, spreadsheets, Parquet), `databases` (SQLite files) or `folders`; without `dir`, the data roots; `parent` empty above a root. Refused (403) in Privacy Mode |
| `POST tables/server/inspect` `{path, format}` | admin | as `tables/inspect`, a table on the server (inside `data_roots`), read where it is, row by row |
| `POST tables/server` `{paths, batch, source, category, mapping, format, dry_run, background}` | admin | tables on the server imported where they are, of any size, as `import-table`: one, or several with the same columns as one database named by `batch`; a relative `file` path is read from the first table's folder |
| `GET import-tasks`, `GET import-tasks/{id}`, `POST import-tasks/{id}/cancel` | admin | imports running in the background (and those ended within the hour): `{id, label, state, started, progress: {phase, rows, done, elapsed_s, rate, left_s, cancelling}, report, error}` (`state`: `running`, `done`, `failed`, `cancelled`; `cancelling`: a stop was asked and is under way); stop one (nothing of it is kept) |
| `GET imports`, `POST imports/{id}/remove` `{purge, dry_run}` | admin | batches; remove one |
| `GET sources`, `POST sources/remove` `{source, purge, dry_run}` | any, admin | `{items, waiting}`: the sources with what they contribute, and the number of items waiting for their encounter; remove one |
| `GET demo/removal` | admin | what deleting the cohort DEMO would remove (a dry run; [administration.md, The demo](administration.md#the-demo)) |
| `GET mappings`, `POST mappings` `{name, kind, columns}`, `DELETE mappings/{name}` | admin | saved column mappings |
| **Connected databases** | | |
| `GET connections`, `POST connections` `{name, kind, url, password, settings}` | admin | the connected databases (no secrets), with the persons linked; connect one (checked first) |
| `POST connections/{id}/check`, `POST connections/{id}/sync`, `DELETE connections/{id}` | admin | whether one answers: `{ok, views, version, ms}` or `{ok: false, error}`; link its persons; disconnect it |
| **Cohorts** | | |
| `POST cohorts/search` `{criteria, source, within_source_of, limit, offset}` | any | the patients meeting the criteria, with why: `{total, items, ids, excluded_total, excluded}` |
| `POST cohorts/parse` `{text}` | any | criteria written as text: `{ok, expr, words}` or `{ok: false, error}` |
| `GET cohorts`, `POST cohorts` `{name, description, criteria, source, patients}` | any | the cohorts one may see, each with its `definition`; make one (`source`: `{op: within \| union \| intersection \| difference, of: [ids]}`; no `patients`: all the definition selects) |
| `GET cohorts/{id}`, `PATCH cohorts/{id}` `{name, description, criteria, add, add_search, remove, shares}`, `DELETE cohorts/{id}` | any (as shared) | a cohort; change it (editors; sharing: its owner); delete it (its owner; the demo's cohort DEMO: an admin, and the demo goes with it: the answer's `removed` says what) |
| `POST cohorts/{id}/clone` `{name}`, `GET cohorts/{id}/history` | any (as shared) | a copy of one's own; its changes, latest first |
| `GET cohorts/{id}/members?limit&offset`, `POST cohorts/{id}/refresh` `{dry_run, join, leave}` | any (as shared) | its members; what its definition selects now (`added`, `removed`; a dry run lists who would join and leave), applied unless `dry_run` |
| `GET cohorts/{id}/export?what&privacy` | any (as shared) | `what`: `members` (CSV, the default), `criteria` (the definition, JSON), `data` (the members' data, CSV), `xlsx`; in Privacy Mode or with `privacy=1`, through the gate |
| `GET cohorts/people` | any | the users a cohort can be shared with (names only) |
| **Work** | | |
| `GET actions`, `POST actions/reload` | any, admin | the actions |
| `POST runs` `{actions, selection, params, force, priority}` | analyst | queue a run. `selection`: `{"encounters": [ids]}`, `{"recordings": [ids]}`, `{"search": {…}}`, `{"cohort": id}` or `{"all": true}` |
| `GET jobs?state&run&limit` | any | jobs and counts per state |
| `POST jobs/cancel` `{job}` or `{run}` (or `{}`: all, admin) | analyst | cancel (a viewer: 403) |
| `GET history`, `DELETE history/{encounter}`, `DELETE history` | any | my history |
| **Users and the audit log** | | |
| `GET users`, `POST users`, `PATCH users/{id}`, `DELETE users/{id}` | admin | users; `POST users` `{username, role, phi, display_name, password}` (with proxy sign-in the password may be left out: the account gets one nobody knows); `PATCH` `{sign_out: true}` ends the user's sessions everywhere (audited) |
| `GET audit?limit`, `GET audit/verify` | admin | the audit log; whether its seals hold: `{seals, entries, unsealed, broken}` |
| **The waveform viewer** | | |
| `GET viewer`, `POST viewer/start` | any | the viewer's status `{mode: embedded\|link\|none, flavor, running, reason, url}`; start it now |

The VitalStream viewer itself is served under `/viewer/`, not under `api/v1`, behind the catalog's
sign-in and its gateway ([administration.md, The waveform viewer](administration.md#the-waveform-viewer)).
The ECG and PACS viewers' pages are served under `/viewers/ecg/` and `/viewers/pacs/`. They hold no
data: they read the files above with the user's session.
