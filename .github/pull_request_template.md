## What and why

<!-- One topic per pull request. Refer to the issue or the docs/todo.md item it addresses. -->

## Checklist

- [ ] I accept the [Contributor License Agreement](../CLA.md) for this contribution.
- [ ] No patient data anywhere in the change: tests, examples, screenshots and fixtures are synthetic ([PRIVACY.md](../PRIVACY.md)); the pre-commit hook is enabled (`git config core.hooksPath tools/git-hooks`).
- [ ] Tests added or updated, and they pass (`make test`, or the part's own: [developing.md](../docs/developing.md#tests)).
- [ ] `make lint` (Rust) and `make lint-python` (no unused imports or undefined names) are clean; the C++ builds without warnings (CI builds it with warnings as errors).
- [ ] The documents that describe the behaviour are updated in the same change ([CONTRIBUTING.md](../CONTRIBUTING.md)).
- [ ] A new dependency is recorded in [THIRD_PARTY_LICENSES.md](../THIRD_PARTY_LICENSES.md) with its licence.
- [ ] Anything that can show, export or store identifiers passes the privacy gate, with a canary in `medorama/tests/privacy.rs`.
