#!/bin/sh
# A stand-in for `docker`, for the launcher's tests. It logs each call to $FAKE_DOCKER/log and
# keeps each container as a file in $FAKE_DOCKER holding its label.
#
# - `image inspect` succeeds, so nothing is built.
# - `ps -aq --filter label=<label>` lists the containers with that label.
# - `rm -f <id>...` removes them, and logs `removed <id>` for each that was there.
# - `run` logs each `-v` mount as `mount <mount>`. With FAKE_DOCKER_SLOW set it logs `creating`
#   and takes a second to create the container. It writes the id to its `--cidfile`, then, if the
#   last argument is `--version`, prints a version and ends; otherwise it runs until removed.
set -u
dir=$FAKE_DOCKER
echo "$*" >>"$dir/log"
case $1 in
image) exit 0 ;;
ps)
    label=${4#label=}
    for f in "$dir"/c-*; do
        [ -e "$f" ] && grep -qxF "$label" "$f" && basename "$f"
    done
    exit 0
    ;;
rm)
    shift 2
    for id in "$@"; do
        [ -e "$dir/$id" ] && rm -f "$dir/$id" && echo "removed $id" >>"$dir/log"
    done
    exit 0
    ;;
run)
    shift
    cid="" label="" last=""
    while [ $# -gt 0 ]; do
        case $1 in
        --cidfile) cid=$2; shift ;;
        --label) label=$2; shift ;;
        -v) echo "mount $2" >>"$dir/log"; shift ;;
        esac
        last=$1
        shift
    done
    if [ -n "${FAKE_DOCKER_SLOW:-}" ]; then
        echo creating >>"$dir/log"
        sleep 1
    fi
    id="c-$$"
    echo "$label" >"$dir/$id"
    [ -z "$cid" ] || printf '%s' "$id" >"$cid"
    if [ "$last" = "--version" ]; then
        echo "0.0.0-fake"
        rm -f "$dir/$id"
        exit 0
    fi
    while [ -e "$dir/$id" ]; do sleep 0.1; done
    exit 0
    ;;
esac
exit 1
