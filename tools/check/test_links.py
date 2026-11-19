#!/usr/bin/env python3
"""Tests for links.py: a check that cannot fail is not a check.

    python3 -m unittest discover -s tools/check -p 'test_*.py'
"""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import links  # noqa: E402


def problems_in(files: dict[str, str]) -> list[str]:
    with tempfile.TemporaryDirectory() as folder:
        root = Path(folder)
        for name, text in files.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")
        return links.check(root)[0]


class Anchors(unittest.TestCase):
    def test_heading_anchors_follow_github(self):
        text = "\n".join(
            [
                "## D-050 The core program is called `marmot-agent`",
                "## D-028 An answer is a request, and the agent's reply settles it",
                "### 3. The agent's reply is what counts",
                "## Q-07 How does a chat map to a harness session?",
                "## A [linked](other.md) heading",
                "## snake_case stays",
                "## _Emphasis_ and __strong__ lose their marks",
            ]
        )
        self.assertEqual(
            links.anchors(text),
            {
                "d-050-the-core-program-is-called-marmot-agent",
                "d-028-an-answer-is-a-request-and-the-agents-reply-settles-it",
                "3-the-agents-reply-is-what-counts",
                "q-07-how-does-a-chat-map-to-a-harness-session",
                "a-linked-heading",
                "snake_case-stays",
                "emphasis-and-strong-lose-their-marks",
            },
        )

    def test_repeated_headings_are_numbered(self):
        self.assertEqual(links.anchors("# Same\n\n## Same\n\n## Same\n"), {"same", "same-1", "same-2"})

    def test_a_repeat_skips_an_anchor_that_is_taken(self):
        self.assertEqual(
            links.anchors("# Install\n# Install\n# Install-1\n"), {"install", "install-1", "install-1-1"}
        )

    def test_html_ids_are_anchors(self):
        self.assertEqual(links.anchors('<a id="hermes-controls"></a>\n'), {"hermes-controls"})

    def test_an_underlined_heading_is_an_anchor(self):
        text = "Installation\n============\n\nSecond part\n---\n"
        self.assertEqual(links.anchors(text), {"installation", "second-part"})

    def test_a_rule_or_a_table_is_not_an_underlined_heading(self):
        text = "A paragraph.\n\n---\n\n| a | b |\n| --- | --- |\n\n- item\n---\n"
        self.assertEqual(links.anchors(text), set())

    def test_a_heading_inside_a_code_block_is_not_an_anchor(self):
        self.assertEqual(links.anchors("```sh\n# not a heading\n```\n# Real\n"), {"real"})


class Links(unittest.TestCase):
    def test_good_links_pass(self):
        self.assertEqual(
            problems_in(
                {
                    "README.md": "# Top\n\n[a](docs/a.md) [b](docs/a.md#part-two) [c](#top) [d](docs)\n"
                    '<img src="docs/pic.svg"> <source srcset="docs/pic.svg">\n'
                    "[web](https://example.com/nothing) [e](docs/a.md#marked)\n\n[ref]: docs/a.md\n",
                    "docs/a.md": '# A\n\n## Part two\n\n<a id="marked"></a>\n\n[up](../README.md#top)\n',
                    "docs/pic.svg": "<svg/>",
                }
            ),
            [],
        )

    def test_a_missing_file_fails(self):
        found = problems_in({"README.md": "line one\n[a](docs/missing.md)\n"})
        self.assertEqual(found, ["README.md:2: docs/missing.md: no such file"])

    def test_a_missing_heading_fails(self):
        found = problems_in({"README.md": "[a](a.md#gone) [b](#gone-too)\n", "a.md": "# Here\n"})
        self.assertEqual(
            found,
            [
                "README.md:1: a.md#gone: no heading '#gone' in a.md",
                "README.md:1: #gone-too: no heading '#gone-too' in README.md",
            ],
        )

    def test_a_missing_image_and_reference_fail(self):
        found = problems_in({"README.md": '<img src="gone.svg">\n\n[ref]: gone.md\n'})
        self.assertEqual(
            found, ["README.md:1: gone.svg: no such file", "README.md:3: gone.md: no such file"]
        )

    def test_a_link_out_of_the_repository_fails(self):
        found = problems_in({"README.md": "[a](../elsewhere.md)\n"})
        self.assertEqual(found, ["README.md:1: ../elsewhere.md: points outside the repository"])

    def test_links_in_code_are_ignored(self):
        self.assertEqual(
            problems_in({"README.md": "`[a](gone.md)`\n\n```text\n[b](gone.md)\n```\n\n~~~\n[c](gone.md)\n~~~\n"}),
            [],
        )

    def test_parentheses_and_spaces_in_a_file_name(self):
        files = {
            "README.md": "[a](guide(v2).md) [b](<release notes.md>) [c](one\\(two.md) [d](gone(v2).md)\n",
            "guide(v2).md": "",
            "release notes.md": "",
            "one(two.md": "",
        }
        self.assertEqual(problems_in(files), ["README.md:1: gone(v2).md: no such file"])

    def test_a_comma_splits_only_a_srcset(self):
        files = {
            "README.md": "[web](https://example.com/a,b) [a](one,two.md)\n"
            '<source srcset="small.svg 1x, gone.svg 2x">\n',
            "one,two.md": "",
            "small.svg": "",
        }
        self.assertEqual(problems_in(files), ["README.md:2: gone.svg: no such file"])

    def test_a_srcset_address_may_hold_a_comma(self):
        self.assertEqual(
            links.srcset_addresses("https://example.com/a,b.svg 1x, two.svg 2x,three.svg 3x , four.svg"),
            ["https://example.com/a,b.svg", "two.svg", "three.svg", "four.svg"],
        )
        self.assertEqual(links.srcset_addresses("one.svg"), ["one.svg"])
        self.assertEqual(links.srcset_addresses("one.svg, two.svg"), ["one.svg", "two.svg"])

    def test_only_whole_attribute_names_count(self):
        files = {"README.md": '<span data-src="not-a-path" data-id="not-an-anchor"></span> [a](#not-an-anchor)\n'}
        self.assertEqual(
            problems_in(files), ["README.md:1: #not-an-anchor: no heading '#not-an-anchor' in README.md"]
        )

    def test_a_fragment_on_a_source_file_is_not_a_heading(self):
        self.assertEqual(problems_in({"README.md": "[a](x.py#L3)\n", "x.py": "pass\n"}), [])


if __name__ == "__main__":
    unittest.main()
