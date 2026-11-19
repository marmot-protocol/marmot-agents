#!/usr/bin/env python3
"""Check the links between the files of this repository.

Usage:
    links.py [root]     check every Markdown file under root (default: the repository)

It reads every Markdown file and checks each link that points inside the repository:

- the file or folder it names exists;
- when it names a heading (`page.md#some-heading` or `#some-heading`), that heading exists.

It covers Markdown links and images, reference definitions, and the `href`, `src` and `srcset`
attributes of HTML in a Markdown file. Headings get the anchors GitHub gives them, and an HTML
`id` or `name` attribute counts as an anchor too.

It does not fetch anything. A link to a web address is not checked, so that the check gives the
same answer on every run and needs no network.

It is not a Markdown parser. It reads one line at a time, so it does not see a link that is
split over two lines, or a reference definition whose address is on the line below its label.
Write a link on one line.

It has no dependencies. It exits with an error if any link is broken.
"""

from __future__ import annotations

import os
import re
import sys
from pathlib import Path
from urllib.parse import unquote

SKIPPED_FOLDERS = {".git", "target", "node_modules", "__pycache__", ".venv"}

FENCE = re.compile(r"^ {0,3}(`{3,}|~{3,})")
HEADING = re.compile(r"^ {0,3}(#{1,6})[ \t]+(.*?)[ \t]*#*[ \t]*$")
CODE_SPAN = re.compile(r"(`+)(.+?)\1")
SETEXT_UNDERLINE = re.compile(r"^ {0,3}(=+|-+)[ \t]*$")
# A destination is `<anything>`, or text with no spaces whose parentheses are escaped or balanced.
DESTINATION = r"<[^>]*>|(?:\\.|\([^()\s]*\)|[^()\s\\])+"
INLINE_LINK = re.compile(r"\]\(\s*(" + DESTINATION + r")(?:\s+(?:\"[^\"]*\"|'[^']*'))?\s*\)")
REFERENCE = re.compile(r"^ {0,3}\[[^\]]+\]:\s*(<[^>]*>|\S+)")
HTML_LINK = re.compile(r"""(?<![\w-])(href|src|srcset)\s*=\s*["']([^"']*)["']""")
ESCAPED = re.compile(r"\\([!-/:-@\[-`{-~])")
HTML_ANCHOR = re.compile(r"""(?<![\w-])(?:id|name)\s*=\s*["']([^"']+)["']""")
SCHEME = re.compile(r"^[A-Za-z][A-Za-z0-9+.-]*:")
MARKDOWN_LINK_TEXT = re.compile(r"!?\[([^\]]*)\]\([^)]*\)")
HTML_TAG = re.compile(r"<[^>]+>")


def slug(heading: str) -> str:
    """The anchor GitHub gives a heading, before duplicates are numbered."""
    text = MARKDOWN_LINK_TEXT.sub(r"\1", heading)
    text = HTML_TAG.sub("", text)
    text = text.strip().lower()
    kept = [c for c in text if c.isalnum() or c in " -_"]
    return "".join(kept).replace(" ", "-")


def lines_outside_code(text: str):
    """Yield (line number, line) for every line that is not inside a fenced code block."""
    fence = None
    for number, line in enumerate(text.splitlines(), start=1):
        match = FENCE.match(line)
        if fence is None:
            if match:
                fence = match.group(1)
                continue
            yield number, line
        elif match and match.group(1)[0] == fence[0] and len(match.group(1)) >= len(fence):
            if line.strip() == match.group(1):
                fence = None


def anchors(text: str) -> set[str]:
    """Every anchor a Markdown file offers."""
    found: set[str] = set()
    seen: dict[str, int] = {}
    above = (0, "")  # the line before this one, for a heading that is underlined
    for number, line in lines_outside_code(text):
        title = None
        heading = HEADING.match(line)
        if heading:
            title = heading.group(2)
        elif SETEXT_UNDERLINE.match(line) and above[0] == number - 1:
            # Under a paragraph line, `===` or `---` makes it a heading. Under a table row, a
            # list item, a quote or another heading it does not.
            text_above = above[1].strip()
            if text_above and text_above[0] not in "|>-*+#=":
                title = text_above
        if title is not None:
            base = slug(title.replace("`", ""))
            count = seen.get(base, 0)
            seen[base] = count + 1
            found.add(base if count == 0 else f"{base}-{count}")
        for anchor in HTML_ANCHOR.findall(CODE_SPAN.sub("", line)):
            found.add(anchor)
        above = (number, line)
    return found


def srcset_addresses(value: str) -> list[str]:
    """The addresses in a `srcset`: candidates split by commas, each an address and then sizes.

    An address can hold a comma itself, so a comma counts only at the end of an address or in
    the sizes after it.
    """
    found = []
    want_address = True
    for token in value.split():
        if want_address:
            address = token.rstrip(",")
            if address:
                found.append(address)
            want_address = token.endswith(",")
            continue
        _, comma, rest = token.partition(",")
        if comma and rest:  # "1x,next.svg": no space after the comma
            found.append(rest.rstrip(","))
            want_address = rest.endswith(",")
        elif comma:
            want_address = True
    return found


def links(text: str):
    """Yield (line number, target) for every link in a Markdown file."""
    for number, line in lines_outside_code(text):
        line = CODE_SPAN.sub("", line)
        targets = list(INLINE_LINK.findall(line))
        reference = REFERENCE.match(line)
        if reference:
            targets.append(reference.group(1))
        for target in targets:
            if target.startswith("<") and target.endswith(">"):
                target = target[1:-1]
            else:
                target = ESCAPED.sub(r"\1", target)
            if target:
                yield number, target
        for attribute, value in HTML_LINK.findall(line):
            if attribute == "srcset":
                candidates = srcset_addresses(value)
            else:
                candidates = [value.strip()] if value.strip() else []
            for target in candidates:
                yield number, target


def markdown_files(root: Path) -> list[Path]:
    found = []
    for folder, folders, files in os.walk(root):
        folders[:] = sorted(f for f in folders if f not in SKIPPED_FOLDERS)
        found.extend(Path(folder) / name for name in sorted(files) if name.endswith(".md"))
    return found


def check(root: Path) -> tuple[list[str], int, int]:
    """Return the problems found, the number of links checked and the number of files read."""
    root = root.resolve()
    files = markdown_files(root)
    texts = {path: path.read_text(encoding="utf-8") for path in files}
    known: dict[Path, set[str]] = {}

    def anchors_of(path: Path) -> set[str]:
        if path not in known:
            text = texts.get(path)
            if text is None:
                text = path.read_text(encoding="utf-8")
            known[path] = anchors(text)
        return known[path]

    problems = []
    checked = 0
    for path in files:
        shown = path.relative_to(root)
        for number, target in links(texts[path]):
            if SCHEME.match(target) or target.startswith("//"):
                continue
            checked += 1
            where, _, fragment = target.partition("#")
            where = unquote(where.split("?")[0])
            fragment = unquote(fragment)
            if not where:
                dest = path
            elif where.startswith("/"):
                dest = (root / where.lstrip("/")).resolve()
            else:
                dest = (path.parent / where).resolve()
            if dest != root and root not in dest.parents:
                problems.append(f"{shown}:{number}: {target}: points outside the repository")
                continue
            if not dest.exists():
                problems.append(f"{shown}:{number}: {target}: no such file")
                continue
            if not fragment:
                continue
            if dest.is_dir():
                dest = dest / "README.md"
                if not dest.exists():
                    problems.append(f"{shown}:{number}: {target}: a folder has no headings")
                    continue
            if dest.suffix != ".md":
                continue  # a line number in a source file, for example
            if fragment not in anchors_of(dest):
                problems.append(
                    f"{shown}:{number}: {target}: no heading '#{fragment}' in {dest.relative_to(root)}"
                )
    return problems, checked, len(files)


def main(argv: list[str]) -> int:
    root = Path(argv[1]) if len(argv) > 1 else Path(__file__).resolve().parents[2]
    problems, checked, files = check(root)
    for problem in problems:
        print(problem)
    if problems:
        print(f"\n{len(problems)} broken of {checked} links in {files} Markdown files")
        return 1
    if not checked:
        print(f"no links found in {files} Markdown files under {root}: wrong folder?")
        return 1
    print(f"{checked} links in {files} Markdown files: all resolve")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
