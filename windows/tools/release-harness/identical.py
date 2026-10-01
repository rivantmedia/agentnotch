#!/usr/bin/env python3
"""Proves the Mac release is built exactly as before Windows existed (DESIGN-WIN R9).

Compares the `mac` job of .github/workflows/release.yml with the single `release` job of
the release.yml that shipped 1.0.1 (fixtures/release-1.0.1.yml, a copy of
`git show agentnotch-v1.0.1:.github/workflows/release.yml`, committed so this runs offline
and keeps meaning the same after the Windows port lands on main):

- the checkout, Select Xcode, Check the update signing key and Build the release steps are
  byte for byte the same, except `steps.plan.outputs.X` -> `needs.plan.outputs.X` (the plan
  moved into a job of its own); every such change is printed;
- the job runs where it did, with the same environment and the same Xcode;
- its outputs hand publish the same build outputs the old later steps read;
- the Mac part of the release notes is the old text, line for line and in one piece;
- the website refresh job keeps its steps, environment and permissions.

Usage: identical.py [--release PATH] [--baseline PATH | --against GIT_REF]
Exit 0 when everything matches, 1 with the differences otherwise, 2 on a usage error.
"""

import difflib
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
sys.path.insert(0, HERE)

import extract  # noqa: E402

COMPARED = ["actions/checkout@v7", "Select Xcode", "Check the update signing key", "Build the release"]
OLD_REF = "steps.plan.outputs."
NEW_REF = "needs.plan.outputs."


def _args(argv):
    opts = {"release": os.path.join(ROOT, ".github", "workflows", "release.yml"),
            "baseline": os.path.join(HERE, "fixtures", "release-1.0.1.yml"), "against": None}
    i = 0
    while i < len(argv):
        if argv[i] in ("--release", "--baseline", "--against") and i + 1 < len(argv):
            opts[argv[i][2:]] = argv[i + 1]
            i += 2
        else:
            raise SystemExit("usage: identical.py [--release PATH] [--baseline PATH | --against GIT_REF]")
    return opts


def _baseline(opts):
    if opts["against"]:
        out = subprocess.run(["git", "-C", ROOT, "show", opts["against"] + ":.github/workflows/release.yml"],
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, universal_newlines=True)
        if out.returncode != 0:
            raise SystemExit("identical.py: git show %s failed: %s" % (opts["against"], out.stderr.strip()))
        return out.stdout, "git show %s" % opts["against"]
    with open(opts["baseline"]) as f:
        return f.read(), os.path.relpath(opts["baseline"], ROOT)


def _notes_lines(run):
    """The lines that write notes.md: from `{` to `} > "$RUNNER_TEMP/notes.md"`, both excluded."""
    lines = run.split("\n")
    start = lines.index("{") + 1
    end = lines.index('} > "$RUNNER_TEMP/notes.md"')
    return lines[start:end]


def _contains_run(haystack, needle):
    for i in range(len(haystack) - len(needle) + 1):
        if haystack[i:i + len(needle)] == needle:
            return True
    return False


def compare(old_text, new_text):
    """[(ok, message)] for every check."""
    results = []
    old_steps = extract.raw_steps(old_text, "release")
    new_steps = extract.raw_steps(new_text, "mac")
    old_by_label = dict(old_steps)
    new_labels = [label for label, _ in new_steps]

    # The compared steps open the mac job, in the old order.
    if new_labels[:len(COMPARED)] == COMPARED:
        results.append((True, "mac job opens with " + ", ".join(COMPARED)))
    else:
        results.append((False, "mac job's first steps are %s, not %s" % (new_labels[:len(COMPARED)], COMPARED)))

    new_by_label = dict(new_steps)
    for label in COMPARED:
        if label not in old_by_label or label not in new_by_label:
            results.append((False, "%s: missing (%s)" % (label, "baseline" if label not in old_by_label else "mac job")))
            continue
        changed = []
        expected = []
        for line in old_by_label[label]:
            moved = line.replace(OLD_REF, NEW_REF)
            if moved != line:
                changed.append(line.strip() + "  ->  " + moved.strip())
            expected.append(moved)
        actual = new_by_label[label]
        if expected == actual:
            note = "; allowed: " + "; ".join(changed) if changed else ""
            results.append((True, "%s: identical%s" % (label, note)))
        else:
            diff = "\n".join(difflib.unified_diff(expected, actual, "baseline (plan refs moved)", "release.yml mac", lineterm="", n=1))
            results.append((False, "%s: differs\n%s" % (label, diff)))

    old = extract.load(old_text).job("release")
    new_wf = extract.load(new_text)
    mac = new_wf.job("mac")
    for key in ("runs_on", "environment"):
        a, b = getattr(old, key), getattr(mac, key)
        results.append((a == b, "mac %s: %r%s" % (key.replace("_", "-"), b, "" if a == b else " (was %r)" % a)))
    a, b = old.env.get("XCODE_APP"), mac.env.get("XCODE_APP")
    results.append((a == b and a is not None, "mac XCODE_APP: %r%s" % (b, "" if a == b else " (was %r)" % a)))
    a, b = old.data.get("timeout-minutes"), mac.data.get("timeout-minutes")
    results.append((a == b, "mac timeout-minutes: %r%s" % (b, "" if a == b else " (was %r)" % a)))

    # What the old later steps read from the build, publish now reads from the job's outputs.
    for name in ("DMG", "ZIP", "NOTARIZED"):
        value = mac.outputs.get(name)
        ok = value == "${{ steps.build.outputs.%s }}" % name
        results.append((ok, "mac output %s: %r" % (name, value)))

    # The website refresh: same steps, same environment and token permission (its OIDC
    # token names release.yml, main and the release environment; the website checks them).
    old_site = extract.raw_steps(old_text, "website")
    new_site = extract.raw_steps(new_text, "website")
    ok = old_site == new_site
    results.append((ok, "website steps: %s" % ("identical" if ok else "differ")))
    a, b = extract.load(old_text).job("website"), new_wf.job("website")
    ok = (a.environment, a.permissions, a.runs_on) == (b.environment, b.permissions, b.runs_on)
    results.append((ok, "website environment, permissions and runner: %s" % ("as before" if ok else "changed")))

    old_notes = old.step("Write the release notes")
    new_notes = new_wf.job("publish").step("Write the release notes")
    old_lines = _notes_lines(old_notes.run)
    new_lines = _notes_lines(new_notes.run)
    ok = _contains_run(new_lines, old_lines)
    results.append((ok, "release notes: the Mac part (%d lines) %s" % (
        len(old_lines), "is the old text in one piece" if ok else "is not the old text")))
    for name in ("DMG", "NOTARIZED"):
        value = new_notes.env.get(name)
        ok = value == "${{ needs.mac.outputs.%s }}" % name
        results.append((ok, "release notes input %s: %r (was %r)" % (name, value, old_notes.env.get(name))))
    return results


def main(argv):
    opts = _args(argv)
    old_text, source = _baseline(opts)
    with open(opts["release"]) as f:
        new_text = f.read()
    results = compare(old_text, new_text)
    print("Mac release steps against %s:" % source)
    failed = 0
    for ok, message in results:
        print("%s %s" % ("ok  " if ok else "FAIL", message))
        failed += 0 if ok else 1
    if failed:
        print("%d check(s) failed: the Mac release must stay as it was (DESIGN-WIN R9)." % failed)
        return 1
    print("The Mac release is built as before.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
