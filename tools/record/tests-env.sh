# shellcheck shell=sh
# The model the recorders run on (D-100), as the scenarios read it: from the environment, or else from
# tests.env at the top of the repository, which git ignores (tests.env.example shows it). Sourced by a
# recorder, which sets TESTS_ENV to that file's path first; there is no default.

# Prints the setting NAME, or says what to set and fails.
tests_env() {
    value=$(printenv "$1" || true)
    if [ -z "$value" ] && [ -f "$TESTS_ENV" ]; then
        value=$(sed -n "s/^[[:space:]]*$1=//p" "$TESTS_ENV" | tail -n 1 | sed 's/[[:space:]]*$//')
    fi
    [ -n "$value" ] || { echo "error: set $1, in the environment or in tests.env (see tests.env.example)" >&2; return 2; }
    printf '%s' "$value"
}
