"""Static checks of the user's manual's LaTeX, without a TeX engine: ASCII source, balanced
environments and braces, escaped special characters, known macros and environments, labels
defined once, every reference defined, listings that fit the page, and every figure file present in
figures/ (run `make figures` first).

    python3 docs/user-manual/check_manual.py          # from the repository root

It prints each problem (file:line: what) and exits with a nonzero status if there is one.
"""
import re
import sys
from pathlib import Path

ROOT = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent
files = [ROOT / 'medorama-user-manual.tex', ROOT / 'preamble.tex'] + sorted((ROOT / 'chapters').glob('*.tex'))

KNOWN = set('''
documentclass usepackage input begin end chapter section subsection subsubsection paragraph part
label ref cref Cref pageref caption centering includegraphics textwidth linewidth item toprule midrule
bottomrule endhead endfirsthead endfoot endlastfoot hline cline multicolumn multirow textbf textit emph
texttt textsf textsuperscript textsubscript ldots dots times approx rightarrow leftarrow pm le ge leq geq
neq to infty frac sqrt cdot quad qquad mathrm mathit mathbf text sum lfloor rfloor lceil rceil
url href footnote newline linebreak pagebreak newpage clearpage smallskip medskip bigskip vspace hspace
hfill vfill noindent par textless textgreater textbar textasciitilde textbackslash textasciicircum
textunderscore textmu textdegree textcopyright code file cmd opt phimark stpversion apientry
small footnotesize scriptsize tiny large Large LARGE huge Huge normalsize normalfont ttfamily rmfamily
sffamily bfseries itshape mdseries upshape raggedright raggedleft arraybackslash tableofcontents
frontmatter mainmatter backmatter appendix addcontentsline title author date maketitle today
nopagebreak allowbreak mbox makebox hookrightarrow space ensuremath displaystyle textcolor color
nolinkurl enspace thinspace negthinspace log ln exp min max lvert rvert vert mid ll gg sim propto
alpha beta gamma delta epsilon mu sigma tau theta lambda pi omega Delta Sigma left right big Big
bigl bigr cdots vdots in notin subset cup cap forall exists lbrace rbrace langle rangle
mathtt mathsf underline overline hat bar tilde ast star circ bullet dagger textendash textemdash
leftmark nouppercase thepage headrulewidth fancyhf fancyhead fancyfoot fancypagestyle pagestyle
setlength setcounter renewcommand newcommand definecolor newcolumntype lstdefinelanguage lstset
lstdefinestyle newtcolorbox hypersetup newlist setlist pdfstringdefDisableCommands let def
stpunderscore setmonofont ifPDFTeX else fi parindent parskip arraystretch captionsetup
titlepage vspace thanks and protect relax ignorespaces strut rule hrulefill dotfill
textquotesingle textquotedbl lq rq S P dag ddag oe ae ss o l i j copyright pounds
equiv Rightarrow Leftarrow Leftrightarrow leftrightarrow mapsto uparrow downarrow
textheight addlinespace hphantom vphantom phantom tabhead mddate mdversion md ui key screenshot returns raises cdot appendix leq geq textbf partname tablename thetable textunderscore texorpdfstring bmod listoffigures pagenumbering thispagestyle markboth fill selectfont fontsize linespread textcopyright
'''.split())

problems = []
labels = {}
refs = []


def p(f, n, msg):
    problems.append(f'{f.name}:{n}: {msg}')


for f in files:
    text = f.read_text(encoding='utf-8', errors='replace')
    raw = f.read_bytes()
    for n, line in enumerate(raw.split(b'\n'), 1):
        if any(b > 126 or (b < 32 and b not in (9, 13)) for b in line):
            p(f, n, 'non-ASCII byte')
    lines = text.split('\n')
    stack = []
    in_lst = False
    brace = 0
    for n, line in enumerate(lines, 1):
        if in_lst:
            if re.match(r'\s*\\end\{lstlisting\}', line):
                in_lst = False
                if not stack or stack[-1][0] != 'lstlisting':
                    p(f, n, 'end lstlisting without begin')
                else:
                    stack.pop()
            elif len(line) > 100:
                p(f, n, f'listing line of {len(line)} chars')
            continue
        # strip comments
        code = re.sub(r'(?<!\\)%.*', '', line)
        if f.name != 'preamble.tex' and re.search(r'(?<!\\)%', line) and not line.lstrip().startswith('%'):
            p(f, n, 'raw % in text (comment?)')
        for m in re.finditer(r'\\(begin|end)\{([^}]*)\}', code):
            kind, env = m.groups()
            if kind == 'begin':
                if env == 'lstlisting':
                    tables = [e for e, _ in stack if e in ('tabularx', 'xltabular', 'tabular', 'longtable')]
                    if tables:
                        p(f, n, f'lstlisting inside {tables[-1]}')
                    stack.append((env, n))
                    rest = code[m.end():]
                    if '\\end{lstlisting}' not in rest:
                        in_lst = True
                    break
                stack.append((env, n))
            else:
                if not stack:
                    p(f, n, f'end {env} without begin')
                elif stack[-1][0] != env:
                    p(f, n, f'end {env} but open {stack[-1][0]} (line {stack[-1][1]})')
                    stack.pop()
                else:
                    stack.pop()
        if in_lst:
            continue
        # braces
        c2 = re.sub(r'\\[{}]', '', code)
        c2 = re.sub(r'\\\\', '', c2)
        brace += c2.count('{') - c2.count('}')
        # math stripped
        nomath = re.sub(r'(?<!\\)\$(?:[^$]|\\\$)*?(?<!\\)\$', 'M', code.replace('\\$', 'D'))
        if nomath.count('$') % 2:
            p(f, n, 'odd $ on line')
        t = re.sub(r'\\(label|cref|Cref|ref|url|includegraphics|input|begin|end|usepackage|documentclass|newcolumntype|lstset|lstdefinelanguage)(\[[^\]]*\])?\{[^}]*\}', '', nomath)
        if f.name not in ('preamble.tex',):
            if re.search(r'(?<!\\)_', t):
                p(f, n, 'unescaped _')
            if re.search(r'(?<!\\)#', t):
                p(f, n, 'unescaped #')
            if re.search(r'(?<!\\)\^', t):
                p(f, n, 'unescaped ^')
            for m in re.finditer(r'\\(code|file|cmd|opt)\{([^{}]*)\}', t):
                if '~' in m.group(2):
                    p(f, n, f'~ inside \\{m.group(1)}')
            amp_ok = any(e in ('tabularx', 'xltabular', 'tabular', 'longtable', 'align', 'array') for e, _ in stack)
            if not amp_ok and re.search(r'(?<!\\)&', t):
                p(f, n, 'unescaped & outside a table')
            if re.search(r'\\item\[[^\]]*\[', t):
                p(f, n, 'nested [ in \\item[...]')
        for m in re.finditer(r'\\([A-Za-z]+)', nomath if f.name != 'preamble.tex' else ''):
            if m.group(1) not in KNOWN:
                p(f, n, f'unknown macro \\{m.group(1)}')
        for m in re.finditer(r'\\label\{([^}]*)\}', code):
            lab = m.group(1)
            if '#' in lab:  # a macro's parameter (preamble)
                continue
            if lab in labels:
                p(f, n, f'duplicate label {lab} (also {labels[lab]})')
            labels[lab] = f'{f.name}:{n}'
            if not re.fullmatch(r'[a-z]+:[a-z0-9-]+', lab):
                p(f, n, f'odd label {lab}')
        for m in re.finditer(r'\\(?:cref|Cref|ref|pageref)\{([^}]*)\}', code):
            for lab in m.group(1).split(','):
                refs.append((f, n, lab.strip()))
        # LaTeX keeps the line break of a list split over two lines: " sec:x" is another label
        if re.search(r'\\(?:cref|Cref|ref|pageref)\{[^}]*$', code):
            p(f, n, 'reference list split over two lines')
    # a figure's label given to \screenshot{file}{caption}{label}, which may span several lines,
    # and the figure files that \screenshot and \includegraphics name
    nocomment = '\n'.join(re.sub(r'(?<!\\)%.*', '', l) for l in lines)
    for m in re.finditer(r'\\screenshot\{([^}]*)\}', nocomment):
        n = nocomment.count('\n', 0, m.start()) + 1
        depth, i, args = 0, m.end(), []
        while len(args) < 2 and i < len(nocomment):
            if nocomment[i] == '{' and nocomment[i - 1] != '\\':
                if depth == 0:
                    start = i + 1
                depth += 1
            elif nocomment[i] == '}' and nocomment[i - 1] != '\\':
                depth -= 1
                if depth == 0:
                    args.append(nocomment[start:i])
            i += 1
        if len(args) == 2 and args[1].startswith('fig:'):
            lab = args[1]
            if lab in labels:
                p(f, n, f'duplicate label {lab} (also {labels[lab]})')
            labels[lab] = f'{f.name}:{n}'
        elif f.name != 'preamble.tex':
            p(f, n, 'screenshot without a fig: label')
    if f.name != 'preamble.tex':
        for m in re.finditer(r'\\(?:screenshot|includegraphics(?:\[[^\]]*\])?)\{([^}#]*)\}', nocomment):
            n = nocomment.count('\n', 0, m.start()) + 1
            if not (ROOT / 'figures' / m.group(1)).exists():
                p(f, n, f'missing figure file figures/{m.group(1)}')
    if stack:
        for env, n in stack:
            p(f, n, f'unclosed {env}')
    if brace:
        p(f, 0, f'brace balance {brace:+d}')

for f, n, lab in refs:
    if lab not in labels:
        p(f, n, f'undefined reference {lab}')

envs_known = {'document', 'titlepage', 'table', 'figure', 'tabularx', 'xltabular', 'tabular', 'itemize',
              'enumerate', 'description', 'params', 'note', 'tip', 'phibox', 'lstlisting', 'center', 'quote',
              'longtable', 'minipage', 'align', 'align*', 'equation', 'equation*', 'array', 'verbatim', 'flushleft',
              'small', 'footnotesize', 'figure*', 'tabular*'}
for f in files:
    for n, line in enumerate(f.read_text().split('\n'), 1):
        for m in re.finditer(r'\\begin\{([^}]*)\}', line):
            if m.group(1) not in envs_known:
                p(f, n, f'unknown environment {m.group(1)}')

print('\n'.join(problems) or 'no problems')
print(f'{len(labels)} labels, {len(refs)} references, {len(problems)} problems')
sys.exit(1 if problems else 0)
