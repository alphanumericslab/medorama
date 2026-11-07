# Medorama user's manual

This folder holds the user's manual of Medorama, written in LaTeX, and explains how to build it
and keep it in step with the code. The manual is for users, administrators and integrators, and
it also serves the institutional IT security reviewers who must judge Medorama before it runs on
patient data. It covers every feature in one book:

- installing and deploying Medorama and each viewer;
- every screen and feature of the page;
- bringing data in: imported tables, connected databases, CLIF, recordings (WFDB, EDF and
  formats read by add-ons), ECGs and imaging;
- the viewers on their own: VitalStream, the ECG viewer and the PACS viewer;
- administration: users and privacy, sign-in (Medorama's passwords or the institution's sign-in
  through a proxy), deployment, add-ons, readers and new formats, troubleshooting;
- architecture and security: the system's parts and trust boundaries, how patient data are
  protected, and the security review;
- a reference of the command line, the HTTP APIs, the settings and the formats.

The manual gathers the repository's Markdown documents ([`docs/`](..) and the READMEs) into one
document. The Markdown documents are the authoritative source, and the manual agrees with them
and with the code.

## Contents of this folder

| File | Content |
|---|---|
| `medorama-user-manual.pdf` | The manual, built from these sources. |
| `medorama-user-manual.tex` | The main file: the title page and the list of parts and chapters, with each chapter's label. |
| `preamble.tex` | The design (fonts, colors, headings, listings, boxes) and the macros. It follows the design of the [wfdb-x](https://github.com/alphanumericslab/wfdb-x) manual, so that the two read alike. |
| `chapters/*.tex` | One file per chapter. |
| `figures/` | Copies of [`../images`](../images): the screenshots as they are, and the diagrams converted to PDF. |
| `Makefile`, `latexmkrc` | The build: `latexmk` with pdfLaTeX, auxiliary files in `build/`. |
| `check_manual.py` | Static checks of the sources, without a TeX engine. |

The manual has seven parts and appendices: getting started; using Medorama; bringing data in;
the viewers on their own; administration (including the chapters on sign-in and on readers and
new formats); architecture and security (the architecture, how patient data are protected, and
the security review); and the reference. The appendices hold the glossary and the licenses.

## Building the PDF

The PDF needs a TeX distribution with the fonts XCharter, Source Sans and Inconsolata: TeX Live
with `texlive-fonts-extra`, MiKTeX, or Overleaf.

The following commands build the PDF, refresh the figures, and remove the auxiliary files:

```sh
cd docs/user-manual
latexmk                       # -> medorama-user-manual.pdf (auxiliary files in build/)
make figures                  # after a screenshot or a diagram of ../images changed (needs rsvg-convert)
make clean
```

You can also run `pdflatex medorama-user-manual.tex` three times, so that the table of contents
and the cross-references resolve.

To build on **Overleaf**:
1. Run `make figures`, then upload this folder: the main file, `preamble.tex`, `chapters/` and
   `figures/`.
2. Set the main document to `medorama-user-manual.tex` and the compiler to pdfLaTeX.
3. Download the PDF and put it back in this folder under the same name.

## Keeping the manual in step with the code

Every change to Medorama, its viewers or their documents is made in the manual too, in the same
change. A new version updates `\mdversion` and `\mddate` in `preamble.tex`.

`check_manual.py` checks the sources without a TeX engine:
- the source is ASCII, and special characters are escaped;
- environments and braces are balanced;
- every macro and environment is known;
- every label is defined once (including the figure labels of `\screenshot`, whose arguments
  may span lines), and every reference is defined;
- every figure that `\screenshot` or `\includegraphics` names exists in `figures/` (run
  `make figures` first).

The following command runs it from the repository's root. It prints each problem and exits with
a nonzero status if there is one:

```sh
python3 docs/user-manual/check_manual.py
```

## Writing conventions

These conventions keep the chapters reading as one manual.

- **Wording.** Professional technical English for a reader who has never seen Medorama:
  complete sentences, active voice, present tense, short paragraphs, American spelling. Each
  chapter opens with a sentence or two on what it covers and for whom. "Medorama" names the
  system; the viewers are named VitalStream, the ECG viewer and the PACS viewer. The labels of
  buttons and tabs are written exactly as the pages show them. The source is ASCII.
- **Markup:**
  - `\code{...}` for identifiers and values (escape `_ # % & { }` as `\_ \# \% \& \{ \}`);
  - `\cmd{...}` for command lines, `\opt{...}` for options, `\file{...}` for files and folders
    (it breaks after each `/`);
  - `\ui{...}` for a button, tab or menu (`\ui{Open record\dots}`), `\key{...}` for a key;
  - `\phimark` after a field that is protected health information.
  - Boxes: `note`, `tip`, and `phibox` for patient data.
  - Tables: `tabularx` or `xltabular` with `booktabs` rules; header cells in `\tabhead`. A long
    table's caption and label go before `\endfirsthead`, never in its repeated head. A table
    cell may be a phrase, and begins with a capital letter.
- **Reference entries.** Each command, request or setting of a reference chapter is one entry:

  ```latex
  \apientry{medorama import-table}{Import a table}
  \begin{lstlisting}[style=signature]
  medorama import-table FILE... [--mapping FILE] [--category CAT] [--dry-run]
  \end{lstlisting}
  What it does, in a few sentences.
  \begin{params}
  \item[--mapping FILE] ...
  \end{params}
  \begin{lstlisting}[style=shell]
  medorama import-table labs.csv --dry-run
  \end{lstlisting}
  ```

- **Screenshots** are inserted with `\screenshot{figure file}{caption}{fig:...}`. Every picture
  shows synthetic data. The page and every viewer have a light and a dark theme; the screenshots
  show the dark theme, except the ECG viewer, which is shown in both (`ecg.png` in the light
  theme, `ecg-dark.png` in the dark one). The figure of the page's dark theme is
  `patient-icu.png`.
- **Chapters** begin with `\chapter{...}` and its `\label{chap:...}` (listed in the main file).
  Sections are labeled `sec:<chapter>-...`, tables `tab:<chapter>-...`, figures
  `fig:<chapter>-...`.
- **Examples** use only the synthetic demo (`tools/make-demo-data.py`) and the synthetic test
  recordings. No real name, identifier, recording name or value ever appears.
- **Claims** match the code. Security claims name the module and the test that enforces them.
  Medorama is research software, not a medical device; the manual never claims certification or
  compliance, and says that Medorama is designed to support HIPAA and GDPR safeguards, the
  deploying institution remaining responsible.
