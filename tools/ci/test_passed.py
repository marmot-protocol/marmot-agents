"""Tests for passed.py, the check behind CI's one required job, `CI passed` (D-130)."""

import unittest

from passed import check, parse_expected

EXPECTED = """
Tier and changed paths
Rust tests (core)
Rust tests (drivers and the rest)
Move-over tests (Docker) [full]
TLA+ models (fair) [models]
TLA+ models (give-up) [models]
"""


def job(name, conclusion="success", status="completed"):
    return {"name": name, "status": status, "conclusion": conclusion}


def pull_request_run():
    """A pull request's run in which the path rule left the models out."""
    return [
        job("Tier and changed paths"),
        job("Rust tests (core)"),
        job("Rust tests (drivers and the rest)"),
        job("Move-over tests (Docker)", "skipped"),
        # A matrix job left out by its `if` is listed once, under its unexpanded name.
        job("TLA+ models (${{ matrix.part }})", "skipped"),
        job("CI passed", None, "in_progress"),
    ]


def full_run():
    return [
        job("Tier and changed paths"),
        job("Rust tests (core)"),
        job("Rust tests (drivers and the rest)"),
        job("Move-over tests (Docker)"),
        job("TLA+ models (fair)"),
        job("TLA+ models (give-up)"),
        job("CI passed", None, "in_progress"),
    ]


def without(jobs, name):
    return [j for j in jobs if j["name"] != name]


def replaced(jobs, name, **changes):
    return [dict(j, **changes) if j["name"] == name else j for j in jobs]


class Passes(unittest.TestCase):
    def test_a_pull_request_whose_tier_and_path_rule_left_jobs_out(self):
        self.assertEqual(check(EXPECTED, pull_request_run(), full=False, models=False), [])

    def test_the_full_gate(self):
        self.assertEqual(check(EXPECTED, full_run(), full=True, models=True), [])

    def test_a_pull_request_that_runs_the_models(self):
        jobs = without(pull_request_run(), "TLA+ models (${{ matrix.part }})")
        jobs += [job("TLA+ models (fair)"), job("TLA+ models (give-up)")]
        self.assertEqual(check(EXPECTED, jobs, full=False, models=True), [])

    def test_a_workflow_call_with_caller_prefix_and_sibling_jobs(self):
        prefixed = [dict(j, name=f"CI / {j['name']}") for j in full_run()]
        prefixed = [job("Check whether master moved")] + prefixed + [job("Fast-forward canary", None, "queued")]
        self.assertEqual(check(EXPECTED, prefixed, full=True, models=True, prefix="CI / "), [])


class Fails(unittest.TestCase):
    def assertFails(self, errors, *words):
        self.assertTrue(errors, "the check passed")
        joined = "\n".join(errors)
        for word in words:
            self.assertIn(word, joined)

    def test_when_a_matrix_leg_never_ran(self):
        # GitHub can report a matrix job as passed to `needs` when a leg was cancelled before a
        # runner took it, and the leg may then be missing from the run's jobs.
        jobs = without(full_run(), "TLA+ models (give-up)")
        self.assertFails(check(EXPECTED, jobs, full=True, models=True), "TLA+ models (give-up)", "never ran")

    def test_when_a_job_never_ran(self):
        jobs = without(pull_request_run(), "Rust tests (core)")
        self.assertFails(check(EXPECTED, jobs, full=False, models=False), "Rust tests (core)", "never ran")

    def test_when_a_job_the_tier_needs_was_skipped(self):
        jobs = replaced(full_run(), "Move-over tests (Docker)", conclusion="skipped")
        self.assertFails(check(EXPECTED, jobs, full=True, models=True), "Move-over tests (Docker)", "skipped")

    def test_when_the_models_were_skipped_though_the_path_rule_wanted_them(self):
        self.assertFails(check(EXPECTED, pull_request_run(), full=False, models=True), "TLA+ models (fair)")

    def test_when_a_job_was_cancelled(self):
        jobs = replaced(full_run(), "TLA+ models (fair)", conclusion="cancelled")
        self.assertFails(check(EXPECTED, jobs, full=True, models=True), "TLA+ models (fair)", "cancelled")

    def test_when_a_job_the_tier_leaves_out_failed_anyway(self):
        jobs = replaced(pull_request_run(), "Move-over tests (Docker)", conclusion="failure")
        self.assertFails(check(EXPECTED, jobs, full=False, models=False), "Move-over tests (Docker)", "failure")

    def test_when_a_job_has_not_finished(self):
        jobs = replaced(full_run(), "Rust tests (core)", status="in_progress", conclusion=None)
        self.assertFails(check(EXPECTED, jobs, full=True, models=True), "Rust tests (core)", "in_progress")

    def test_when_a_job_is_missing_from_the_list(self):
        jobs = full_run() + [job("A new job")]
        self.assertFails(check(EXPECTED, jobs, full=True, models=True), "A new job", "not in the list")

    def test_when_a_name_appears_twice_and_one_failed(self):
        jobs = full_run() + [job("Rust tests (core)", "failure")]
        self.assertFails(check(EXPECTED, jobs, full=True, models=True), "Rust tests (core)", "failure")

    def test_when_no_job_matches_the_workflow_call_prefix(self):
        self.assertFails(
            check(EXPECTED, full_run(), full=True, models=True, prefix="Wrong / "),
            "Wrong / ",
            "matched prefix",
        )


class TheList(unittest.TestCase):
    def test_an_unknown_condition_is_an_error(self):
        with self.assertRaises(ValueError):
            parse_expected("Simulator [nightly]")

    def test_an_empty_list_is_an_error(self):
        with self.assertRaises(ValueError):
            parse_expected("\n\n")


if __name__ == "__main__":
    unittest.main()
