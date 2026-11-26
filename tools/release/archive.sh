# shellcheck shell=bash
# Shared deterministic tarball writer for tools/release/package.sh and package_connector.sh.
# Same archive from the same files: sorted entries, numeric zero owners, commit timestamp.
write_release_tarball() {
    local root=$1 work=$2 name=$3 out=$4 mtime
    mtime=$(git -C "$root" log -1 --format=%ct 2>/dev/null || date +%s)
    if tar --version 2>/dev/null | grep -q GNU; then
        tar -C "$work" --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$mtime" -cf - "$name" |
            gzip -n >"$out/$name.tar.gz"
    else
        tar -C "$work" -cf - "$name" | gzip -n >"$out/$name.tar.gz"
    fi
}
