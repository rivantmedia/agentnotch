/**
 * Scripts/cloud-contract-e2e.sh's helpers, sourced into bash (sourced, the script only defines
 * them, under its own `set -euo pipefail`). Docker is a fake on PATH that logs its calls: nothing
 * here reaches a real one.
 */
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

const SCRIPT = path.resolve(
  import.meta.dirname,
  "../../../Scripts/cloud-contract-e2e.sh",
);
const CONTAINER = "agentnotch-web-test-e2e";

let dir: string;
let bin: string;

beforeEach(() => {
  dir = mkdtempSync(path.join(tmpdir(), "contract-e2e-script-"));
  bin = path.join(dir, "bin");
  mkdirSync(bin);
});

afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
});

/** Runs `body` in bash after sourcing the script, with only the fake bin and the system's own. */
function bash(body: string, ...args: string[]) {
  const result = spawnSync(
    "/bin/bash",
    ["-c", `source "$SCRIPT"\n${body}`, "bash", ...args],
    {
      encoding: "utf8",
      env: {
        NODE_ENV: "test",
        PATH: `${bin}:/usr/bin:/bin`,
        SCRIPT,
        HOME: dir,
        TMPDIR: dir,
      },
    },
  );
  return { status: result.status, stderr: result.stderr };
}

/**
 * A docker that logs each call to `calls.log`, and has the contract check's container while
 * `present` exists (`rm -f` of it deletes that file).
 */
function fakeDocker() {
  const calls = path.join(dir, "calls.log");
  const present = path.join(dir, "present");
  const docker = path.join(bin, "docker");
  writeFileSync(
    docker,
    `#!/bin/bash
echo "$*" >> "${calls}"
case "$1" in
  ps) [ -e "${present}" ] && echo "${CONTAINER}"; exit 0 ;;
  rm) [ "$3" = "${CONTAINER}" ] && rm -f "${present}"; exit 0 ;;
esac
exit 0
`,
  );
  chmodSync(docker, 0o755);
  return {
    calls: () =>
      existsSync(calls)
        ? readFileSync(calls, "utf8").trim().split("\n").filter(Boolean)
        : [],
    setPresent: () => writeFileSync(present, ""),
    isPresent: () => existsSync(present),
  };
}

describe("swift_log_passed", () => {
  const test = "theAppsSyncRequestKeepsTheContract";
  const passed = `\u001b[1;32m✔\u001b[0m Test ${test}() passed after 0.041 seconds.\n`;

  function log(text: string): string {
    const file = path.join(dir, "swift.log");
    writeFileSync(file, text);
    return file;
  }

  it("finds the test's pass, colours and all", () => {
    expect(bash(`swift_log_passed ${test} "$1"`, log(passed)).status).toBe(0);
  });

  it("takes a pass followed by any amount of log as a pass (no SIGPIPE under pipefail)", () => {
    // Far more than a pipe and sed's buffer hold after the match: a reader that stops at the
    // match leaves sed writing into a closed pipe.
    const after = `\u001b[1;32m✔\u001b[0m Suite CloudContractE2ETests passed after 0.042 seconds.\n`;
    const file = log(`build output\n${passed}${after.repeat(10_000)}`);
    for (let run = 0; run < 2; run += 1) {
      expect(bash(`swift_log_passed ${test} "$1"`, file).status).toBe(0);
    }
  });

  it("refuses a log where the test failed, didn't run or is missing", () => {
    for (const text of [
      `✘ Test ${test}() failed after 0.041 seconds with 1 issue.\n`,
      "✔ Test theWebsitesSyncResponseIsRead() passed after 0.01 seconds.\n",
      "✔ Test run with 0 tests in 0 suites passed after 0.001 seconds.\n",
      "",
    ]) {
      expect(bash(`swift_log_passed ${test} "$1"`, log(text)).status).not.toBe(
        0,
      );
    }
    expect(
      bash(`swift_log_passed ${test} "$1"`, path.join(dir, "none.log")).status,
    ).not.toBe(0);
  });
});

describe("the exit trap", () => {
  it("removes the contract check's container when the web runner died without stopping it", () => {
    const docker = fakeDocker();
    docker.setPresent();
    // The runner was SIGKILLed during step 2: the script exits 137 with the container running.
    const run = bash(`WORK="$(mktemp -d)"; trap cleanup EXIT; exit 137`);
    expect(run.status).toBe(137);
    expect(docker.isPresent()).toBe(false);
    expect(docker.calls()).toEqual([
      `ps -a --filter name=^/${CONTAINER}$ --format {{.Names}}`,
      `rm -f ${CONTAINER}`,
    ]);
    expect(run.stderr).toContain(`removed the leftover ${CONTAINER}`);
    expect(run.stderr).toContain("FAILED");
  });

  it("only looks when the runner stopped it, and never names another container", () => {
    const docker = fakeDocker();
    const run = bash(
      `WORK="$(mktemp -d)"; echo "$WORK" > "$1"; trap cleanup EXIT; exit 0`,
      path.join(dir, "work-path"),
    );
    expect(run.status).toBe(0);
    expect(docker.calls()).toEqual([
      `ps -a --filter name=^/${CONTAINER}$ --format {{.Names}}`,
    ]);
    // A passing run leaves nothing behind.
    const work = readFileSync(path.join(dir, "work-path"), "utf8").trim();
    expect(existsSync(work)).toBe(false);
  });

  it("does without Docker", () => {
    const run = bash(`WORK="$(mktemp -d)"; trap cleanup EXIT; exit 1`);
    expect(run.status).toBe(1);
    expect(run.stderr).toContain("FAILED");
  });
});
