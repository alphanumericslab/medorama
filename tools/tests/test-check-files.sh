#!/usr/bin/env bash
# Tests of the privacy check (tools/git-hooks/check-files) through the hooks, in a scratch
# repository: what is refused and what is let through.  Run: bash tools/tests/test-check-files.sh
set -eu

HOOKS=$(cd "$(dirname "$0")/../git-hooks" && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
cd "$WORK"
git init -q -b main repo
cd repo
git config user.name test
git config user.email test@example.org
git config commit.gpgsign false
git config core.hooksPath "$HOOKS"
echo base > README
git add README
git commit -qm base

# recording names, put together here so that this file holds none itself
T=1700000000
NAME="ICU_BED12-$T" SPACED="CVICU_BED 7-$T"

passed=0 failed=0
ok() { passed=$((passed + 1)); }
bad() { failed=$((failed + 1)); echo "FAIL: $1" >&2; }

# refused NAME MESSAGE: committing what is staged fails and names MESSAGE; the index is reset
refused() {
  if git commit -qm "$1" 2> "$WORK/err"; then
    bad "$1: committed"
    git reset -q --hard HEAD~1
  elif ! grep -qF -- "$2" "$WORK/err"; then
    bad "$1: refused without \"$2\": $(cat "$WORK/err")"
  else
    ok
  fi
  git reset -q
}
# accepted NAME: committing what is staged succeeds
accepted() {
  if git commit -qm "$1" 2> "$WORK/err"; then ok; else bad "$1: refused: $(cat "$WORK/err")"; fi
}
add() { mkdir -p "$(dirname "$1")"; printf '%b' "$2" > "$1"; git add -- "$1"; }

# the path and the content
add "$NAME.txt" 'hello\n';                      refused "name in path" "the path holds a recording name"
add "$SPACED.txt" 'hello\n';                    refused "name with a space in path" "the path holds a recording name"
add q.txt "$SPACED\\n";                         refused "name with a space" "contains a recording name"
add b.bin "$NAME\\000\\001\\002";                  refused "name in a binary file" "contains a recording name"
add medorama/sample_data/x.txt 'x\n';            refused "nested sample_data" "sample_data/ folders"
add sample_data/notes.txt 'x\n';                 refused "sample_data" "sample_data/ folders"

# the staged content, not the working tree's
add p.txt "$NAME\\n"; printf 'clean\n' > p.txt
refused "staged name, clean working tree" "contains a recording name"
add c.txt 'clean\n'; printf '%s\n' "$NAME" > c.txt
accepted "clean staged, name in the working tree"
head -c 6000000 /dev/zero > big.txt; git add big.txt; printf 'small\n' > big.txt
refused "large staged, small working tree" "5.7 MB is larger than 5 MB"
rm -f big.txt
# the user's manual may be up to 20 MB; another PDF may not
mkdir -p docs/user-manual
head -c 6000000 /dev/zero > docs/user-manual/medorama-user-manual.pdf; git add docs/user-manual
accepted "the manual over 5 MB"
head -c 6000000 /dev/zero > docs/other.pdf; git add docs/other.pdf
refused "another PDF over 5 MB" "5.7 MB is larger than 5 MB"
rm -f docs/other.pdf

# a symbolic link is checked by its path, not by what it points to
if ln -s "$WORK/huge" link.txt 2> /dev/null; then
  head -c 6000000 /dev/zero > "$WORK/huge"
  git add link.txt; accepted "a link to a large file"
fi

# data types, whatever their case, outside the synthetic test data
add catalog.sqlite-wal 'x';                       refused "sqlite-wal" "data file type"
add catalog.db-wal 'x';                           refused "db-wal" "data file type"
add dump.sql 'x';                                 refused "sql dump" "data file type"
add pts.csv 'mrn,name\n123,Jane\n';               refused "csv" "data file type"
add labs.XLSX 'x';                                refused "xlsx" "data file type"
add export.zip 'PK';                              refused "zip" "data file type"
add export.tar.gz 'x';                            refused "gz" "data file type"
add rec.EDF 'x';                                  refused "EDF" "data file type"
add medorama/src/schema/sqlite/009_more.sql 'CREATE TABLE t (x);\n'; accepted "the catalog's schema"
add viewers/vitalstream/testdata/real.EDF 'x';    accepted "test data EDF"
add medorama/tests/fixtures/t.csv 'a\n1\n';       accepted "fixture csv"

# DICOM and MUSE XML by their content
{ head -c 128 /dev/zero; printf 'DICM'; } > image.bin; git add image.bin
refused "DICOM with another extension" "a DICOM file"
add img.dcm 'DICM';                               refused "dcm" "data file type"
add ecg.xml '<?xml version="1.0"?>\n<RestingECG><PatientID>123</PatientID></RestingECG>\n'
refused "MUSE XML" "GE MUSE XML ECG"
add ecg2.txt '<RestingECG>\n';                    refused "MUSE XML named txt" "GE MUSE XML ECG"
add parse.js 'const root = "<RestingECG>";\n';    accepted "code naming the element"
add viewers/ecg/examples/e.xml '<RestingECG/>\n'; accepted "an example MUSE XML"

# notebooks
add n.ipynb '{"cells":[{"cell_type":"code","outputs":[1]}],\n'; refused "malformed notebook" "not a readable notebook"
add N2.IPYNB '{"cells":[{"cell_type":"code","outputs":[{"text":"x"}]}]}'; refused "notebook with outputs" "saved outputs"
add clean.ipynb '{"cells":[{"cell_type":"code","outputs":[]}],"metadata":{}}'; accepted "clean notebook"

# a merge brings in what was committed without the hook (pre-merge-commit)
git checkout -qb side
add side.txt "$NAME\\n"; git commit -qm side --no-verify
git checkout -q main
add main.txt 'm\n'; git commit -qm m
if git merge -q --no-edit side 2> "$WORK/err"; then bad "merge: committed"
elif grep -qF "contains a recording name" "$WORK/err"; then ok; else bad "merge: $(cat "$WORK/err")"; fi
git merge --abort

# CI: every commit of a range, including a file removed again later
git checkout -q side
git rm -q side.txt; git commit -qm "removed again"
if "$HOOKS/check-files" --commits main..side 2> "$WORK/err"; then bad "--commits: passed"
elif grep -qF "side.txt (commit" "$WORK/err"; then ok; else bad "--commits: $(cat "$WORK/err")"; fi
if "$HOOKS/check-files" --tree side 2> "$WORK/err"; then ok; else bad "--tree: $(cat "$WORK/err")"; fi
git checkout -q main
if "$HOOKS/check-files" --commits main..main 2> "$WORK/err"; then ok; else bad "--commits empty: $(cat "$WORK/err")"; fi
if "$HOOKS/check-files" --tree no-such-rev 2> /dev/null; then bad "--tree no-such-rev: passed"; else ok; fi

echo "check-files: $passed passed, $failed failed"
[ "$failed" -eq 0 ]
