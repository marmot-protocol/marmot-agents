#!/usr/bin/env python3
"""Generate the design diagrams as SVG, in a light and a dark variant.

Run from the repository root:

    python3 docs/images/generate.py

It has no dependencies. Each diagram is written twice, as `<name>-light.svg`
and `<name>-dark.svg`, next to this script. Markdown embeds the pair with a
`<picture>` element so GitHub shows the one matching the reader's theme.

Edit a diagram by changing its function below and re-running the script. Do
not edit the SVG files by hand.
"""

from __future__ import annotations

from pathlib import Path
from xml.sax.saxutils import escape

OUT = Path(__file__).resolve().parent

SANS = "-apple-system, BlinkMacSystemFont, 'Segoe UI', 'Noto Sans', Helvetica, Arial, sans-serif"
MONO = "ui-monospace, SFMono-Regular, 'SF Mono', Menlo, Consolas, monospace"

THEMES = {
    "light": {
        "bg": "#ffffff",
        "ink": "#1f2328",
        "muted": "#59636e",
        "line": "#6e7781",
        "box_fill": "#f6f8fa",
        "box_stroke": "#d0d7de",
        "frame_stroke": "#afb8c1",
        "shared_fill": "#ddf4ff",
        "shared_stroke": "#0969da",
        "specific_fill": "#fff8c5",
        "specific_stroke": "#9a6700",
    },
    "dark": {
        "bg": "#0d1117",
        "ink": "#e6edf3",
        "muted": "#9198a1",
        "line": "#8b949e",
        "box_fill": "#161b22",
        "box_stroke": "#30363d",
        "frame_stroke": "#484f58",
        "shared_fill": "#121d2f",
        "shared_stroke": "#4493f8",
        "specific_fill": "#272115",
        "specific_stroke": "#d29922",
    },
}


class Svg:
    """A very small SVG builder: boxes, text, and arrows on a pixel grid."""

    def __init__(self, width: int, height: int, theme: dict[str, str], label: str):
        self.w, self.h, self.t = width, height, theme
        self.label = label
        self.parts: list[str] = []

    # -- primitives ---------------------------------------------------------

    def rect(self, x, y, w, h, kind="box", rx=8, dashed=False):
        fill = self.t.get(f"{kind}_fill", "none")
        stroke = self.t.get(f"{kind}_stroke", self.t["box_stroke"])
        dash = ' stroke-dasharray="6 5"' if dashed else ""
        self.parts.append(
            f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" '
            f'fill="{fill}" stroke="{stroke}" stroke-width="1.5"{dash}/>'
        )

    def text(self, x, y, s, size=13, weight=400, color="ink", anchor="start", mono=False):
        family = MONO if mono else SANS
        self.parts.append(
            f'<text x="{x}" y="{y}" font-family="{family}" font-size="{size}" '
            f'font-weight="{weight}" fill="{self.t[color]}" text-anchor="{anchor}">{escape(s)}</text>'
        )

    def line(self, points, start_arrow=False, end_arrow=True):
        d = " ".join(f"{x},{y}" for x, y in points)
        markers = ""
        if end_arrow:
            markers += ' marker-end="url(#arrow)"'
        if start_arrow:
            markers += ' marker-start="url(#arrow)"'
        self.parts.append(
            f'<polyline points="{d}" fill="none" stroke="{self.t["line"]}" stroke-width="1.5"{markers}/>'
        )

    # -- composites ---------------------------------------------------------

    def box(self, x, y, w, h, title, lines=(), kind="box", title_size=13, line_size=11):
        """A box with a bold centred title and optional muted lines under it."""
        self.rect(x, y, w, h, kind=kind)
        gap = line_size + 4
        block = title_size + len(lines) * gap
        top = y + (h - block) / 2 + title_size - 2
        self.text(x + w / 2, round(top, 1), title, size=title_size, weight=600, anchor="middle")
        for i, s in enumerate(lines):
            self.text(
                x + w / 2, round(top + (i + 1) * gap, 1), s, size=line_size, color="muted", anchor="middle"
            )

    def frame(self, x, y, w, h, title):
        """A dashed container with its title in the top-left corner."""
        self.rect(x, y, w, h, kind="frame", rx=12, dashed=True)
        self.text(x + 14, y + 22, title, size=12, weight=600, color="muted")

    # -- output -------------------------------------------------------------

    def render(self) -> str:
        t = self.t
        head = (
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {self.w} {self.h}" '
            f'width="{self.w}" height="{self.h}" role="img" aria-label="{escape(self.label)}">\n'
            f"<title>{escape(self.label)}</title>\n"
            "<defs>"
            '<marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" '
            'markerHeight="7" orient="auto-start-reverse">'
            f'<path d="M0,0 L10,5 L0,10 z" fill="{t["line"]}"/></marker>'
            "</defs>\n"
            f'<rect width="{self.w}" height="{self.h}" fill="{t["bg"]}"/>\n'
        )
        return head + "\n".join(self.parts) + "\n</svg>\n"


def legend(s: Svg, x: int, y: int):
    s.rect(x, y, 14, 14, kind="shared", rx=3)
    s.text(x + 22, y + 11.5, "written once", size=11.5, color="muted")
    s.rect(x + 118, y, 14, 14, kind="specific", rx=3)
    s.text(x + 140, y + 11.5, "one per harness", size=11.5, color="muted")


# -- diagram 1: where everything runs ----------------------------------------


def overview(theme) -> Svg:
    s = Svg(
        1024,
        560,
        theme,
        "People in a Marmot group reach agent harnesses through one core program. "
        "Codex, Pi and ACP harnesses, Cursor among them, use drivers built into the core. Claude Code, "
        "Hermes and OpenClaw connect as separate programs over a local socket.",
    )

    # People and relays, on the left.
    s.frame(20, 170, 196, 230, "Marmot group chat")
    s.box(36, 206, 164, 54, "You", ["the agent's owner"])
    s.box(36, 274, 164, 54, "Other members", ["see all agent activity"])
    s.text(118, 364, "end-to-end encrypted", size=11, color="muted", anchor="middle")
    s.text(118, 380, "White Noise apps", size=11, color="muted", anchor="middle")

    s.box(250, 257, 112, 56, "Nostr relays", ["ciphertext only"])
    s.line([(216, 285), (250, 285)], start_arrow=True)
    s.line([(362, 285), (412, 285)], start_arrow=True)

    # The host machine.
    s.frame(392, 30, 616, 510, "Agent host: the machine your agents run on")

    # The core program.
    s.rect(412, 150, 226, 270, kind="shared", rx=10)
    s.text(525, 176, "marmot-agents core", size=14, weight=700, anchor="middle")
    s.text(525, 193, "one Rust program", size=11, color="muted", anchor="middle")
    s.box(426, 208, 198, 88, "Marmot side", ["MDK as a library:", "account, encryption,", "relays, storage"])
    s.box(
        426, 310, 198, 96, "Conversation handling", ["who may prompt or approve,", "which session a chat maps to,", "turn order"]
    )

    # Drivers built into the core.
    rows_a = [
        (104, "Codex driver", "app-server JSON-RPC", "Codex", None),
        (152, "Pi driver", "RPC mode", "Pi", None),
        (200, "ACP driver", "Agent Client Protocol", "Goose, OpenCode", "Cursor, other ACP agents"),
    ]
    s.text(690, 90, "Built into the core · Rust · one driver trait", size=11.5, weight=600, color="muted")
    # Separate programs over the socket.
    rows_b = [
        (296, "Claude driver", "TypeScript · official SDK", "Claude Code", None),
        (344, "Hermes plugin", "Python · runs inside Hermes", "Hermes", None),
        (392, "OpenClaw plugin", "TypeScript · runs inside it", "OpenClaw", None),
    ]
    s.text(690, 282, "Separate programs · over the local socket", size=11.5, weight=600, color="muted")

    for rows in (rows_a, rows_b):
        for y, title, sub, harness, harness_sub in rows:
            s.box(690, y, 156, 40, title, [sub], kind="specific", title_size=12.5, line_size=10.5)
            lines = [harness_sub] if harness_sub else []
            s.box(868, y, 128, 40, harness, lines, title_size=12 if harness_sub else 12.5, line_size=9.5)
            s.line([(846, y + 20), (868, y + 20)], start_arrow=True)
        centres = [y + 20 for y, *_ in rows]
        mid = centres[len(centres) // 2] if len(centres) % 2 else (centres[0] + centres[-1]) // 2
        s.line([(638, mid), (664, mid)], start_arrow=True, end_arrow=False)
        s.line([(664, centres[0]), (664, centres[-1])], end_arrow=False)
        for c in centres:
            s.line([(664, c), (690, c)], start_arrow=False)

    legend(s, 412, 506)
    return s


# -- diagram 2: the four layers ----------------------------------------------


def layers(theme) -> Svg:
    s = Svg(
        1024,
        250,
        theme,
        "Four layers sit between a harness and the group chat. The driver and the translation "
        "are written once per harness. Conversation handling and the Marmot side are written once.",
    )
    y, h = 86, 120
    s.box(12, y, 98, h, "Harness", ["Claude Code,", "Codex, Hermes,", "and so on"])
    s.box(134, y, 176, h, "1  Driver", ["speaks the harness's own", "interface and holds", "the live session"], kind="specific")
    s.box(334, y, 176, h, "2  Translation", ["turns native events into", "Marmot items, and", "answers back again"], kind="specific")
    s.box(534, y, 200, h, "3  Conversation handling", ["who may prompt or approve,", "which session a chat uses,", "turn order"], kind="shared")
    s.box(758, y, 156, h, "4  Marmot side", ["account, encryption,", "relays, storage", "(MDK)"], kind="shared")
    s.box(938, y, 74, h, "Group", ["chat"])
    for x1, x2 in [(110, 134), (310, 334), (510, 534), (734, 758), (914, 938)]:
        s.line([(x1, y + h / 2), (x2, y + h / 2)], start_arrow=True)

    # Brackets naming what is per harness and what is shared.
    for x1, x2, kind, label in [
        (134, 510, "specific", "one per harness"),
        (534, 914, "shared", "written once"),
    ]:
        stroke = theme[f"{kind}_stroke"]
        s.parts.append(
            f'<path d="M{x1},{y - 14} L{x1},{y - 24} L{x2},{y - 24} L{x2},{y - 14}" fill="none" '
            f'stroke="{stroke}" stroke-width="1.5"/>'
        )
        s.text((x1 + x2) / 2, y - 34, label, size=12.5, weight=600, anchor="middle")
    s.text(
        512, y + h + 30, "Everything a harness says travels left to right. Everything a person does travels right to left.",
        size=11.5, color="muted", anchor="middle",
    )
    return s


# -- diagram 3: one event, three parts ---------------------------------------


def event(theme) -> Svg:
    s = Svg(
        1000,
        330,
        theme,
        "A native harness event becomes one Marmot agent event with three parts: an item type "
        "from the shared vocabulary, a plain rendering any client can show, and the untouched "
        "native payload.",
    )
    # Native event.
    s.rect(16, 62, 250, 206)
    s.text(141, 46, "What the harness sends", size=12.5, weight=600, anchor="middle")
    for i, ln in enumerate(
        [
            "{",
            '  "type": "commandExecution",',
            '  "command": "cargo test",',
            '  "status": "completed",',
            '  "exitCode": 0,',
            '  "durationMs": 8412,',
            '  "aggregatedOutput": "..."',
            "}",
        ]
    ):
        indent = len(ln) - len(ln.lstrip(" "))
        s.text(30 + indent * 7, 92 + i * 21, ln.lstrip(" "), size=11.5, mono=True)
    s.text(141, 290, "an illustrative Codex event", size=11, color="muted", anchor="middle")

    s.line([(266, 165), (350, 165)])
    s.text(308, 155, "translate", size=11.5, color="muted", anchor="middle")

    # The Marmot event.
    s.frame(350, 30, 320, 270, "One Marmot agent event")
    s.box(366, 64, 288, 64, "Item type: tool, a command", ["from the shared vocabulary"], kind="shared")
    s.box(366, 140, 288, 64, "Plain rendering", ["“Ran cargo test · finished · exit 0”"], kind="shared")
    s.box(366, 216, 288, 64, "Native payload", ["the harness's event, untouched"], kind="specific")

    # Clients.
    s.box(744, 66, 244, 84, "A client that knows this item", ["draws a rich command card,", "using the native payload"])
    s.box(744, 190, 244, 84, "Any other client", ["shows the plain rendering,", "so nothing is unreadable"])
    s.line([(670, 108), (744, 108)])
    s.line([(670, 232), (744, 232)])
    return s


# -- diagram 4: one item, and the edits that update it --------------------------


def carrier(theme) -> Svg:
    s = Svg(
        1000,
        300,
        theme,
        "An agent sends an item, then two edits that point at it. "
        "The chat shows one row, in place, holding the latest edit.",
    )
    s.text(180, 40, "What the agent sends, over time", size=12.5, weight=600, anchor="middle")
    rows = [
        (60, "Agent item", "Working"),
        (136, "Edit of that item · version 2", "Working · ran 3 commands"),
        (212, "Edit of that item · version 3", "Finished · ran 12 commands, edited 3 files"),
    ]
    for y, title, sub in rows:
        s.box(16, y, 328, 58, title, [sub], kind="specific" if y == 212 else "box")
    s.line([(344, 241), (430, 241), (430, 179), (580, 179)])
    s.text(452, 232, "each edit points", size=11.5, color="muted")
    s.text(452, 247, "at the item and", size=11.5, color="muted")
    s.text(452, 262, "replaces it", size=11.5, color="muted")

    s.frame(560, 50, 424, 220, "What the chat shows")
    s.box(580, 96, 384, 44, "Alice: run the tests and fix what fails", [], title_size=12)
    s.box(580, 150, 384, 58, "One row, updated in place", ["Finished · ran 12 commands, edited 3 files"], kind="shared")
    s.text(772, 236, "The highest version wins.", size=11.5, color="muted", anchor="middle")
    s.text(772, 252, "The row stays where the item first appeared.", size=11.5, color="muted", anchor="middle")
    return s


# -- diagram 5: an answer, and the agent's reply ---------------------------------


def answer(theme) -> Svg:
    s = Svg(
        1000,
        316,
        theme,
        "A person presses Allow once. Their app sends an answer that points at the approval item. "
        "The agent checks it, passes it to the harness, and edits the approval item to say who answered. "
        "The edit is what every member's app shows.",
    )
    s.box(16, 112, 196, 76, "Alice presses Allow once", ["her app sends an answer"])
    s.line([(212, 150), (300, 150)])
    s.text(256, 128, "answer", size=11.5, color="muted", anchor="middle")
    s.text(256, 142, "kind 1202", size=11.5, color="muted", anchor="middle")
    s.text(256, 170, "points at", size=11.5, color="muted", anchor="middle")
    s.text(256, 184, "the item", size=11.5, color="muted", anchor="middle")

    s.frame(300, 30, 420, 250, "The agent's machine")
    s.box(316, 112, 170, 76, "Core", ["checks that Alice", "may approve"], kind="shared")
    s.box(506, 64, 198, 76, "Driver and harness", ["the harness gets its", "own kind of answer"], kind="specific")
    s.box(506, 184, 198, 76, "Edit of the approval item", ["Answered: Allow once,", "by Alice"], kind="shared")
    s.line([(486, 132), (506, 112)])
    s.line([(605, 140), (605, 184)])

    s.line([(704, 222), (752, 222), (752, 150), (790, 150)])
    s.text(760, 240, "edit", size=11.5, color="muted")
    s.text(760, 254, "kind 1009", size=11.5, color="muted")
    s.box(790, 104, 194, 92, "Every member's chat", ["the approval row reads", "Allowed once by Alice"])
    s.text(500, 304, "The press changes nothing by itself. The agent's edit is what counts.", size=11.5, color="muted", anchor="middle")
    return s


# -- diagram 6: the socket, and what travels each way ----------------------------


def socket(theme) -> Svg:
    s = Svg(
        1000,
        320,
        theme,
        "A connector and the core exchange messages over a local socket. The core sends prompts, answers "
        "and controls. The connector sends agent events and, for agents that act in the chat, chat requests. "
        "Keys, relays and group state stay in the core.",
    )
    s.frame(16, 30, 300, 270, "A connector, its own program")
    s.box(32, 70, 268, 70, "The harness", ["Claude Code, Hermes, OpenClaw"], kind="specific")
    s.box(32, 170, 268, 100, "Connector code", ["turns native messages into agent events,", "and answers into the harness's own calls"], kind="specific")
    s.line([(166, 140), (166, 170)], start_arrow=True)

    s.frame(684, 30, 300, 270, "The core")
    s.box(700, 70, 268, 70, "Conversation handling", ["who may prompt and approve, sessions"], kind="shared")
    s.box(700, 170, 268, 100, "The Marmot side", ["keys, groups, relays, storage"], kind="shared")
    s.line([(834, 140), (834, 170)], start_arrow=True)

    rows = [
        (84, "prompts, answers, controls", False),
        (144, "agent events, and the result of each request", True),
        (204, "chat requests: send, react, read history", True),
    ]
    for y, label, to_core in rows:
        if to_core:
            s.line([(316, y), (684, y)])
        else:
            s.line([(684, y), (316, y)])
        s.text(500, y - 9, label, size=12, anchor="middle")
    s.text(500, 250, "One JSON message per line, on a socket", size=11.5, color="muted", anchor="middle")
    s.text(500, 266, "that only this user can open.", size=11.5, color="muted", anchor="middle")
    return s


DIAGRAMS = {
    "overview": overview,
    "layers": layers,
    "event": event,
    "carrier": carrier,
    "answer": answer,
    "socket": socket,
}


def main() -> None:
    for name, build in DIAGRAMS.items():
        for theme_name, theme in THEMES.items():
            path = OUT / f"{name}-{theme_name}.svg"
            path.write_text(build(theme).render(), encoding="utf-8")
            print(f"wrote {path.relative_to(OUT.parent.parent)}")


if __name__ == "__main__":
    main()
