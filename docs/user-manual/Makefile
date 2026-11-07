# The user's manual: `make` builds medorama-user-manual.pdf (latexmk, pdflatex), `make figures`
# copies the screenshots and converts the diagrams of ../images (rsvg-convert), `make clean`
# removes the auxiliary files.
.PHONY: pdf figures clean
pdf:
	latexmk
figures:
	cp ../images/screens/*.png figures/
	for f in ../images/*.svg; do rsvg-convert -f pdf -o figures/$$(basename $${f%.svg}).pdf $$f; done
clean:
	latexmk -c
	rm -rf build
