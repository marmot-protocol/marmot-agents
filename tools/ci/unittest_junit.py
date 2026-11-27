#!/usr/bin/env python3
"""Run Python's unittest discovery and write a minimal JUnit XML report.

Standard-library only: runs the same discovery and terminal output as
`python3 -m unittest discover`, and writes only structured test identifiers
(`classname`, `name`, `time`) plus empty `<failure/>` / `<error/>` markers to
`--junit`. Stdout, stderr, assertion messages, tracebacks and subTest parameter
values are never written to the XML (D-143, D-171).
"""

from __future__ import annotations

import argparse
import os
import re
import sys
import time
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path

ERROR_HOLDER_RE = re.compile(r"^([A-Za-z0-9_]+)\s*\(([^()]+)\)$")


def _test_identity(test: unittest.case.TestCase) -> tuple[str, str]:
    """Return `(classname, name)` for a unittest case or suite error holder."""
    method = getattr(test, "_testMethodName", None)
    if isinstance(method, str) and method:
        cls = test.__class__
        return f"{cls.__module__}.{cls.__qualname__}", method
    raw_id = ""
    try:
        raw_id = str(test.id())
    except Exception:
        raw_id = test.__class__.__qualname__
    match = ERROR_HOLDER_RE.match(raw_id.strip())
    if match:
        return match.group(2).strip(), match.group(1).strip()
    return test.__class__.__module__, raw_id.strip() or test.__class__.__qualname__


class JUnitTextTestResult(unittest.TextTestResult):
    """TextTestResult that records per-test outcomes without capturing test output."""

    def __init__(self, stream, descriptions, verbosity, *, durations=None):
        try:
            super().__init__(stream, descriptions, verbosity, durations=durations)
        except TypeError:
            super().__init__(stream, descriptions, verbosity)
        self._started: dict[int, float] = {}
        self._records: dict[int, tuple[object, dict]] = {}
        self.junit_cases: list[dict] = []

    def _new_record(self, test) -> dict:
        classname, name = _test_identity(test)
        record = {
            "classname": classname,
            "name": name,
            "time": 0.0,
            "status": "passed",
        }
        self.junit_cases.append(record)
        return record

    def _record_for(self, test) -> dict:
        entry = self._records.get(id(test))
        if entry is not None and entry[0] is test:
            return entry[1]
        return self._new_record(test)

    def startTest(self, test):
        key = id(test)
        self._started[key] = time.monotonic()
        self._records[key] = (test, self._new_record(test))
        super().startTest(test)

    def stopTest(self, test):
        key = id(test)
        started = self._started.pop(key, None)
        entry = self._records.pop(key, None)
        if started is not None and entry is not None and entry[0] is test:
            entry[1]["time"] = max(0.0, time.monotonic() - started)
        super().stopTest(test)

    def addError(self, test, err):
        self._record_for(test)["status"] = "error"
        super().addError(test, err)

    def addFailure(self, test, err):
        rec = self._record_for(test)
        if rec["status"] != "error":
            rec["status"] = "failure"
        super().addFailure(test, err)

    def addSkip(self, test, reason):
        rec = self._record_for(test)
        if rec["status"] == "passed":
            rec["status"] = "skipped"
        super().addSkip(test, reason)

    def addUnexpectedSuccess(self, test):
        rec = self._record_for(test)
        if rec["status"] != "error":
            rec["status"] = "failure"
        super().addUnexpectedSuccess(test)

    def addSubTest(self, test, subtest, err):
        if err is not None:
            rec = self._record_for(test)
            exc_type = err[0] if isinstance(err, tuple) and err else None
            if exc_type is not None and issubclass(exc_type, test.failureException):
                if rec["status"] != "error":
                    rec["status"] = "failure"
            else:
                rec["status"] = "error"
        super().addSubTest(test, subtest, err)


def write_junit(path: Path, suite_name: str, cases: list[dict], elapsed: float) -> None:
    """Write minimal JUnit XML containing only test identifiers and empty failure/error tags."""
    failures = sum(1 for c in cases if c["status"] == "failure")
    errors = sum(1 for c in cases if c["status"] == "error")
    skipped = sum(1 for c in cases if c["status"] == "skipped")

    root = ET.Element("testsuites")
    suite = ET.SubElement(
        root,
        "testsuite",
        {
            "name": suite_name,
            "tests": str(len(cases)),
            "failures": str(failures),
            "errors": str(errors),
            "skipped": str(skipped),
            "time": f"{elapsed:.6f}",
        },
    )
    for case in cases:
        tc = ET.SubElement(
            suite,
            "testcase",
            {
                "classname": case["classname"],
                "name": case["name"],
                "time": f"{case['time']:.6f}",
            },
        )
        if case["status"] == "failure":
            ET.SubElement(tc, "failure", {"type": "failure"})
        elif case["status"] == "error":
            ET.SubElement(tc, "error", {"type": "error"})
        elif case["status"] == "skipped":
            ET.SubElement(tc, "skipped")

    path.parent.mkdir(parents=True, exist_ok=True)
    tree = ET.ElementTree(root)
    if hasattr(ET, "indent"):
        ET.indent(tree, space="  ")
    tree.write(path, encoding="utf-8", xml_declaration=True)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--junit", required=True, type=Path, help="output JUnit XML file path")
    parser.add_argument("-s", "--start-directory", default=".", help="directory to start discovery")
    parser.add_argument("-p", "--pattern", default="test*.py", help="pattern to match test files")
    parser.add_argument("-t", "--top-level-directory", default=None, help="top level directory of project")
    parser.add_argument("-v", "--verbose", action="store_true", help="verbose output")
    parser.add_argument("--durations", type=int, default=None, help="show N slowest test cases (Python 3.12+)")
    args = parser.parse_args(argv)

    # Match `python3 -m unittest`: put the current working directory on sys.path so suites in
    # subdirectories (such as `cd python && ... -s tests`) can import packages from cwd.
    cwd = os.getcwd()
    if cwd not in sys.path:
        sys.path.insert(0, cwd)

    loader = unittest.TestLoader()
    suite = loader.discover(
        start_dir=args.start_directory,
        pattern=args.pattern,
        top_level_dir=args.top_level_directory,
    )

    runner_kwargs: dict = {
        "verbosity": 2 if args.verbose else 1,
        "resultclass": JUnitTextTestResult,
    }
    if args.durations is not None:
        runner_kwargs["durations"] = args.durations

    start = time.monotonic()
    try:
        runner = unittest.TextTestRunner(**runner_kwargs)
    except TypeError:
        runner_kwargs.pop("durations", None)
        runner = unittest.TextTestRunner(**runner_kwargs)

    result = runner.run(suite)
    elapsed = max(0.0, time.monotonic() - start)
    suite_name = os.path.normpath(args.start_directory)
    write_junit(args.junit, suite_name, result.junit_cases, elapsed)
    if not result.wasSuccessful():
        return 1
    if result.testsRun == 0 and not result.skipped:
        return 5
    return 0


if __name__ == "__main__":
    sys.exit(main())
