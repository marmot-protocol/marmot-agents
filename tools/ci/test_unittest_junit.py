"""Tests for tools/ci/unittest_junit.py (D-171, #184)."""

from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "unittest_junit.py"


class UnittestJUnitTests(unittest.TestCase):
    def _run_suite(self, test_source: str, extra_args: list[str] | None = None) -> tuple[int, str, Path]:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        (root / "test_sample.py").write_text(test_source, encoding="utf-8")
        junit = root / "out" / "junit.xml"
        cmd = [
            sys.executable,
            str(SCRIPT),
            "--junit",
            str(junit),
            "-s",
            str(root),
            "-p",
            "test_*.py",
            *(extra_args or []),
        ]
        proc = subprocess.run(cmd, capture_output=True, text=True, check=False)
        self.assertTrue(junit.is_file(), f"missing JUnit XML; stderr:\n{proc.stderr}")
        return proc.returncode, junit.read_text(encoding="utf-8"), junit

    def test_passing_suite_exits_zero_and_records_all_cases(self):
        rc, xml_text, _ = self._run_suite(
            """
import unittest

class Sample(unittest.TestCase):
    def test_one(self):
        self.assertEqual(1, 1)

    @unittest.skip("not now")
    def test_skipped(self):
        pass
"""
        )
        self.assertEqual(rc, 0)
        root = ET.fromstring(xml_text)
        suite = root.find("testsuite")
        self.assertIsNotNone(suite)
        self.assertEqual(suite.get("tests"), "2")
        self.assertEqual(suite.get("failures"), "0")
        self.assertEqual(suite.get("errors"), "0")
        self.assertEqual(suite.get("skipped"), "1")
        names = {(tc.get("classname"), tc.get("name")) for tc in root.iter("testcase")}
        self.assertEqual(
            names,
            {("test_sample.Sample", "test_one"), ("test_sample.Sample", "test_skipped")},
        )

    def test_failures_errors_subtests_and_setupclass_are_recorded_without_output_or_messages(self):
        rc, xml_text, _ = self._run_suite(
            """
import sys
import unittest

class Broken(unittest.TestCase):
    def test_fail(self):
        print("CANARY_STDOUT_SECRET")
        print("CANARY_STDERR_SECRET", file=sys.stderr)
        self.fail("CANARY_ASSERTION_SECRET")

    def test_error(self):
        raise RuntimeError("CANARY_EXCEPTION_SECRET")

    def test_subtest_fail(self):
        with self.subTest(secret="CANARY_SUBTEST_PARAM"):
            self.assertEqual(1, 2, "CANARY_SUBTEST_MSG")

class BrokenSetup(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        raise ValueError("CANARY_SETUPCLASS_SECRET")

    def test_never_runs(self):
        pass
""",
            extra_args=["-v", "--durations", "5"],
        )
        self.assertEqual(rc, 1)
        for leaked in (
            "CANARY_STDOUT_SECRET",
            "CANARY_STDERR_SECRET",
            "CANARY_ASSERTION_SECRET",
            "CANARY_EXCEPTION_SECRET",
            "CANARY_SUBTEST_PARAM",
            "CANARY_SUBTEST_MSG",
            "CANARY_SETUPCLASS_SECRET",
        ):
            self.assertNotIn(leaked, xml_text)

        root = ET.fromstring(xml_text)
        outcomes = {}
        for tc in root.iter("testcase"):
            key = (tc.get("classname"), tc.get("name"))
            tags = [child.tag for child in tc]
            outcomes[key] = tags

        self.assertEqual(outcomes[("test_sample.Broken", "test_fail")], ["failure"])
        self.assertEqual(outcomes[("test_sample.Broken", "test_error")], ["error"])
        self.assertEqual(outcomes[("test_sample.Broken", "test_subtest_fail")], ["failure"])
        self.assertEqual(outcomes[("test_sample.BrokenSetup", "setUpClass")], ["error"])

    def test_discovery_puts_cwd_on_sys_path_like_python_m_unittest(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        pkg = root / "mypkg"
        pkg.mkdir()
        (pkg / "__init__.py").write_text("VALUE = 42\n", encoding="utf-8")
        tests = root / "tests"
        tests.mkdir()
        (tests / "test_pkg.py").write_text(
            "import unittest\nimport mypkg\n\n"
            "class PkgTests(unittest.TestCase):\n"
            "    def test_import(self):\n"
            "        self.assertEqual(mypkg.VALUE, 42)\n",
            encoding="utf-8",
        )
        junit = root / "junit.xml"
        proc = subprocess.run(
            [sys.executable, str(SCRIPT), "--junit", str(junit), "-s", "tests"],
            cwd=str(root),
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        root_el = ET.fromstring(junit.read_text(encoding="utf-8"))
        self.assertEqual(root_el.find("testsuite").get("tests"), "1")


if __name__ == "__main__":
    unittest.main()
