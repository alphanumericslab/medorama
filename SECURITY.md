# Security policy

This page explains how to report a security problem in Medorama or its viewers, which versions
receive fixes, and how Medorama is secured in brief. The full description, written for an
institution's security review, is [security and privacy](docs/security.md).

## Report a vulnerability

Please report security problems **privately**, never in a public issue. Use GitHub's private
vulnerability reporting: on this repository's **Security** tab, choose **Report a
vulnerability** (<https://github.com/alphanumericslab/medorama/security/advisories/new>). Only
the maintainer, Reza Sameni, sees the report.

Include what you found, how to reproduce it, and what it exposes. You will receive an answer
within a week. Fixes are released as soon as they are ready, and reporters are credited unless
they prefer otherwise.

Examples of what to report:

- a way to read identifiers in Privacy Mode;
- a way to read files outside the configured data folders;
- a way to bypass sign-in or a role;
- a way to run code through a crafted file.

**Do not send patient data**, real or suspected, in a report. Describe the problem with
synthetic data instead; `tools/make-demo-data.py` makes some.

## Supported versions

Medorama is at version 0.1.0 and has not been released yet. Fixes go to the `main` branch.

## How Medorama is secured

[Security and privacy](docs/security.md) describes the trust boundaries, the threat model,
authentication and authorization, Privacy Mode and the privacy gate, data at rest and in
transit, the audit log, a regulatory mapping, a deployment checklist and the known limitations,
each with the code and the tests that implement it. In brief:

- **Local by default.** The server listens on 127.0.0.1:8090. TLS is provided by a reverse proxy
  or an SSH tunnel. Nothing is sent to any host other than the institution's own databases. The
  catalog is SQLite on one server, or PostgreSQL shared by several servers, in which case TLS is
  required to any other host, with its certificate and name verified, and passwords are read
  from files and never stored.
- **Sign-in in one module** ([sign-in](docs/sign-in.md)). Either Medorama's own accounts sign
  users in, with Argon2id password hashes, 256-bit session tokens stored hashed in `HttpOnly`,
  `SameSite=Strict` cookies, and sign-in throttling; or an institution's authenticating reverse
  proxy does, and Medorama accepts its user header only from the proxy's addresses. Requests that
  change data must carry the header `X-Medorama: 1`, which a page on another site cannot add
  (protection against cross-site request forgery).
- **Roles and a separate PHI permission.** Each user has a role (viewer, analyst or admin), and
  only users with the PHI permission may see identifiers.
- **Privacy Mode,** the default for every user except administrators. The server replaces
  identifiers with placeholders and random pseudonyms, moves dates back, de-identifies WFDB, EDF,
  GE MUSE XML and DICOM headers, and refuses any answer it did not clear. An end-to-end test
  checks that no identifier reaches the browser ([protecting patient data](docs/protecting-patient-data.md)).
- **Data sources are only read.** Connected databases (OMOP, warehouses) are read through
  read-only sessions. The catalog's database is readable by its owner only and can be encrypted
  (SQLCipher). Every access to identifiers is audited, in a tamper-evident log.
- **Separate viewers.** VitalStream, the ECG viewer and the PACS viewer are separate programs and
  pages. The VitalStream viewer that Medorama runs listens on 127.0.0.1 with a private password,
  behind a gateway that denies every request it has no rule for.

The standalone VitalStream viewers (`vitalstream-py`, `vitalstream-rs` and `vitalstream-cpp`)
listen on 127.0.0.1, serve only paths inside their root folder and data roots, and can require
a password, stored as a salted scrypt hash in a file that only its owner can read. They parse
recordings as untrusted input ([VitalStream](viewers/vitalstream/README.md)).

Medorama is research software, not a medical device. It is designed to support HIPAA and GDPR
safeguards, but it is not certified against any standard, and the deploying institution remains
responsible for its deployment.
