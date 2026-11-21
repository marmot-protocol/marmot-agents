#!/bin/sh
# Starts Hermes with the Marmot plugin. On the first start of a Hermes home it writes Hermes's
# configuration, replacing the one the image's build left; later starts keep it, so that what a chat
# changed survives a restart. The model key is read from its mounted file at every start and written
# nowhere.
#
# Needs:
#   /run/secrets/model_key           the model provider's API key (a mounted file, never an image layer)
#   /run/marmot-agent/socket         the core's socket folder, and /run/marmot-agent/connector.token
#   MODEL_BASE_URL, MODEL_NAME       an OpenAI-compatible endpoint, and the model to use
#   REASONING_EFFORT                 optional; the reasoning effort Hermes asks for (default: high)
#   HERMES_CONFIG_PATCH              optional; a JSON object merged into the configuration last
set -eu

: "${MODEL_BASE_URL:?set MODEL_BASE_URL}"
: "${MODEL_NAME:?set MODEL_NAME}"
key_file=/run/secrets/model_key
[ -r "$key_file" ] || { echo "error: mount the model key at $key_file" >&2; exit 2; }

mkdir -p "$HERMES_HOME/plugins"
rm -rf "$HERMES_HOME/plugins/marmot"
cp -r /opt/marmot-plugin "$HERMES_HOME/plugins/marmot"

umask 077
# The key goes to Hermes in its environment only. The configuration names the variable (`key_env`),
# never the key, so no file in Hermes's home holds it, a new key takes effect at the next start, and
# Hermes keeps it from the commands it runs, as it does every variable a `key_env` names.
MARMOT_MODEL_KEY=$(cat "$key_file")
export MARMOT_MODEL_KEY
python - "$HERMES_HOME/marmot/configured" <<'EOF'
import json, os, pathlib, sys
import hermes_yaml

home = pathlib.Path(os.environ["HERMES_HOME"])
config_path = home / "config.yaml"
configured = pathlib.Path(sys.argv[1])

if not configured.exists():
    config = {
        "model": {"default": os.environ["MODEL_NAME"], "provider": "custom",
                  "base_url": os.environ["MODEL_BASE_URL"], "key_env": "MARMOT_MODEL_KEY"},
        "agent": {"reasoning_effort": os.environ.get("REASONING_EFFORT", "high")},
        "platforms": {"marmot": {"enabled": True}},
        # The core decides who may prompt and who may approve (D-010), so Hermes takes whatever reaches
        # it (provisional, Q-22): every user is allowed (MARMOT_ALLOW_ALL_USERS), anyone in the chat may
        # answer an approval, and a person who may start a new session is not asked to confirm it.
        # Hermes's own approval mode is "manual": a command it flags is always put to a person, never
        # decided by a second model.
        "approvals": {"mode": "manual", "approvers": ["*"], "destructive_slash_confirm": False},
    }

    def merge(a, b):
        for k, v in b.items():
            if isinstance(v, dict) and isinstance(a.get(k), dict):
                merge(a[k], v)
            else:
                a[k] = v
        return a

    if os.environ.get("HERMES_CONFIG_PATCH"):
        merge(config, json.loads(os.environ["HERMES_CONFIG_PATCH"]))
    # JSON is valid YAML, and needs no YAML library to write.
    config_path.write_text(json.dumps(config, indent=2) + "\n")
    configured.parent.mkdir(parents=True, exist_ok=True)
    configured.touch()

# An earlier version of this script wrote the key itself into the configuration, and Hermes copies the
# configuration into backups: take it out of each, and point the model at the variable instead.
copies = [p for p in home.glob("config.yaml.*") if p.is_file()]
if (home / "backups" / "config").is_dir():
    copies += [p for p in (home / "backups" / "config").iterdir() if p.is_file()]
for path in [config_path, *copies]:
    if not path.is_file() or "api_key" not in path.read_text(errors="replace"):
        continue
    try:
        config = hermes_yaml.safe_load(path.read_text())
    except Exception:
        config = None
    model = config.get("model") if isinstance(config, dict) else None
    if isinstance(model, dict) and "api_key" in model:
        del model["api_key"]
        model.setdefault("key_env", "MARMOT_MODEL_KEY")
        path.write_text(hermes_yaml.safe_dump(config))
    elif path != config_path and not isinstance(model, dict):
        # A backup that cannot be read may hold the key: Hermes can do without it.
        path.unlink()
EOF

hermes plugins enable marmot || true
exec hermes gateway run
