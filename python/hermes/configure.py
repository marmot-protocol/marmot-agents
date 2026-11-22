"""Hermes's configuration, as the container's entrypoint (entrypoint.sh) sets it at each start.

    python configure.py <the file that marks a Hermes home as configured>

On the first start of a Hermes home it writes the configuration, replacing the one the image's build
left; later starts keep it, so that what a chat changed survives a restart. At every start it takes the
model key out of the configuration and its backups, where an earlier version of the entrypoint wrote
it: the configuration names the variable that holds the key (`key_env`), never the key. And at every
start it points Hermes's commands at the work folder the container mounts (MARMOT_WORK).
"""

import json
import os
import pathlib
import sys

KEY_ENV = "MARMOT_MODEL_KEY"


def first_configuration() -> dict:
    config = {
        "model": {"default": os.environ["MODEL_NAME"], "provider": "custom",
                  "base_url": os.environ["MODEL_BASE_URL"], "key_env": KEY_ENV},
        "agent": {"reasoning_effort": os.environ.get("REASONING_EFFORT", "high")},
        "platforms": {"marmot": {"enabled": True}},
        # The core decides who may prompt and who may approve (D-010), so Hermes takes whatever reaches
        # it (provisional, Q-22): every user is allowed (MARMOT_ALLOW_ALL_USERS), anyone in the chat may
        # answer an approval, and a person who may start a new session is not asked to confirm it.
        # Hermes's own approval mode is "manual": a command it flags is always put to a person, never
        # decided by a second model.
        "approvals": {"mode": "manual", "approvers": ["*"], "destructive_slash_confirm": False},
    }
    if os.environ.get("HERMES_CONFIG_PATCH"):
        merge(config, json.loads(os.environ["HERMES_CONFIG_PATCH"]))
    return config


def merge(a: dict, b: dict) -> dict:
    for k, v in b.items():
        if isinstance(v, dict) and isinstance(a.get(k), dict):
            merge(a[k], v)
        else:
            a[k] = v
    return a


def write(path: pathlib.Path, text: str) -> None:
    """Replaces a file whole, readable by its owner only."""
    new = path.with_name(path.name + ".marmot-new")
    fd = os.open(new, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as file:
        file.write(text)
    os.replace(new, path)


def without_key_lines(text: str, key: str) -> str:
    """The text without each line that names `api_key` or holds the key, nor the lines indented
    below such a line, which may continue its value."""
    kept = []
    below = None
    for line in text.splitlines(keepends=True):
        indent = len(line) - len(line.lstrip(" \t"))
        if below is not None:
            if not line.strip() or indent > below:
                continue
            below = None
        if "api_key" in line or (key and key in line):
            below = indent
            continue
        kept.append(line)
    return "".join(kept)


def scrub(home: pathlib.Path, key: str, yaml) -> None:
    """Takes the model key out of Hermes's configuration and its backups."""
    key = key if len(key) >= 8 else ""  # too short to look for by itself
    config_path = home / "config.yaml"
    copies = [p for p in home.glob("config.yaml.*") if p.is_file()]
    if (home / "backups" / "config").is_dir():
        copies += [p for p in (home / "backups" / "config").iterdir() if p.is_file()]
    for path in [config_path, *copies]:
        if not path.is_file():
            continue
        text = path.read_text(errors="replace")
        if "api_key" not in text and not (key and key in text):
            continue
        try:
            config = yaml.safe_load(text)
        except Exception:
            config = None
        model = config.get("model") if isinstance(config, dict) else None
        if isinstance(model, dict) and "api_key" in model:
            del model["api_key"]
            model.setdefault("key_env", KEY_ENV)
            write(path, yaml.safe_dump(config))
        elif path != config_path:
            if not isinstance(model, dict):
                # A backup that cannot be read may hold the key: Hermes can do without it.
                path.unlink()
        elif config is None:
            # The configuration itself cannot be read, so its key cannot be taken out by its place in
            # it: take out the lines that may hold it. Hermes then reads what is left, as it would
            # have read the whole.
            write(path, without_key_lines(text, key))
            print("hermes-entrypoint: the configuration could not be read; its api_key lines were taken out",
                  file=sys.stderr)


def work(home: pathlib.Path, folder: str, yaml) -> None:
    """Points Hermes's commands at the work folder (terminal.cwd), at the path the container mounts it
    at, which is the core's: at every start, as the folder may move between them. A configuration that
    cannot be read is left as it is."""
    config_path = home / "config.yaml"
    if not folder or not config_path.is_file():
        return
    try:
        config = yaml.safe_load(config_path.read_text())
    except Exception:
        return
    if not isinstance(config, dict):
        return
    terminal = config.setdefault("terminal", {})
    if not isinstance(terminal, dict) or terminal.get("cwd") == folder:
        return
    terminal["cwd"] = folder
    write(config_path, yaml.safe_dump(config))


def main() -> None:
    import hermes_yaml

    home = pathlib.Path(os.environ["HERMES_HOME"])
    configured = pathlib.Path(sys.argv[1])
    if not configured.exists():
        # JSON is valid YAML.
        write(home / "config.yaml", json.dumps(first_configuration(), indent=2) + "\n")
        configured.parent.mkdir(parents=True, exist_ok=True)
        configured.touch()
    scrub(home, os.environ.get(KEY_ENV, "").strip(), hermes_yaml)
    work(home, os.environ.get("MARMOT_WORK", ""), hermes_yaml)


if __name__ == "__main__":
    main()
