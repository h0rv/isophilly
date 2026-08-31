// @ts-check

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, mkdir, readFile, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  CANONICAL_VISUAL_ZOOMS,
  binaryFlagSetting,
  childHasExited,
  drainGrowingTasks,
  freezeRecords,
  integerListSetting,
  integerSetting,
  pngEvidence,
  publishSuccessAfterTeardown,
  publishSuccessEvidence,
  safeRunComponent,
  stopChild,
  validateCitySectorTargets,
  validateReleaseZooms,
  validateTileResponseSnapshot,
  writeJsonAtomic,
} from "../scripts/visual-check-lib.mjs";

test("integer settings use the fallback and reject ambiguous input", () => {
  assert.equal(integerSetting(undefined, 3107, "PORT", 1, 65_535), 3107);
  assert.equal(integerSetting("3108", 3107, "PORT", 1, 65_535), 3108);
  for (const invalid of ["", " 3108", "+3108", "3.1", "0", "65536"]) {
    assert.throws(
      () => integerSetting(invalid, 3107, "PORT", 1, 65_535),
      /invalid PORT/,
    );
  }
});

test("binary flags reject typos instead of silently changing audit scope", () => {
  assert.equal(binaryFlagSetting(undefined, false, "RELEASE"), false);
  assert.equal(binaryFlagSetting("0", true, "RELEASE"), false);
  assert.equal(binaryFlagSetting("1", false, "RELEASE"), true);
  for (const invalid of ["", "true", "false", "yes", "2", " 1"]) {
    assert.throws(() => binaryFlagSetting(invalid, false, "RELEASE"), /expected 0 or 1/);
  }
});

test("integer-list settings reject partial, spaced, and empty zoom values", () => {
  assert.deepEqual(integerListSetting(undefined, [3, 5], "ZOOMS", 0, 10), [3, 5]);
  assert.deepEqual(integerListSetting("0,3,10", [5], "ZOOMS", 0, 10), [0, 3, 10]);
  for (const invalid of [
    "",
    "3,",
    ",3",
    "3,,5",
    "3.5",
    "3x",
    " 3",
    "3, 5",
    "+3",
    "11",
    "3,3",
  ]) {
    assert.throws(() => integerListSetting(invalid, [5], "ZOOMS", 0, 10), /invalid ZOOMS/);
  }
});

test("release evidence requires the canonical zoom matrix", () => {
  assert.doesNotThrow(() => validateReleaseZooms([...CANONICAL_VISUAL_ZOOMS], true));
  assert.doesNotThrow(() => validateReleaseZooms([5], false));
  assert.throws(() => validateReleaseZooms([3, 4, 5], true), /requires canonical zooms/);
  assert.throws(
    () => validateReleaseZooms([3, 4, 5, 7, 10, 9], true),
    /requires canonical zooms/,
  );
});

test("growing response tasks drain through a stable fixed point", async () => {
  const order = [];
  const tasks = [];
  tasks.push(
    Promise.resolve().then(() => {
      order.push("first");
      tasks.push(Promise.resolve().then(() => order.push("second")));
    }),
  );
  await drainGrowingTasks(tasks);
  assert.deepEqual(order, ["first", "second"]);
  assert.equal(tasks.length, 2);
});

test("signal termination counts as an exited child", () => {
  assert.equal(childHasExited({ exitCode: null, signalCode: null }), false);
  assert.equal(childHasExited({ exitCode: 0, signalCode: null }), true);
  assert.equal(childHasExited({ exitCode: 1, signalCode: null }), true);
  assert.equal(childHasExited({ exitCode: null, signalCode: "SIGTERM" }), true);
  assert.equal(childHasExited({ exitCode: null, signalCode: "SIGKILL" }), true);
});

test("bounded child shutdown escalates and fails if SIGKILL does not exit", async () => {
  const signals = [];
  const child = {
    exitCode: null,
    signalCode: null,
    kill(signal) {
      signals.push(signal);
      return true;
    },
  };
  const waits = [false, true];
  await stopChild(child, async () => waits.shift() ?? false, 1);
  assert.deepEqual(signals, ["SIGTERM", "SIGKILL"]);

  signals.length = 0;
  await assert.rejects(
    stopChild(child, async () => false, 1),
    /did not exit after SIGKILL/,
  );
  assert.deepEqual(signals, ["SIGTERM", "SIGKILL"]);
});

test("a frozen response snapshot cannot drift with its source", () => {
  const source = [{ cache: "disk", bytes: 10 }];
  const snapshot = freezeRecords(source);
  source[0].bytes = 20;
  source.push({ cache: "rendered", bytes: 30 });
  assert.deepEqual(snapshot, [{ cache: "disk", bytes: 10 }]);
  assert.equal(Object.isFrozen(snapshot), true);
  assert.equal(Object.isFrozen(snapshot[0]), true);
});

test("tile response evidence must be nonempty, successful, and sized", () => {
  assert.doesNotThrow(() => validateTileResponseSnapshot([{ status: 200, bytes: 0 }]));
  assert.throws(() => validateTileResponseSnapshot([]), /no tile responses/);
  for (const response of [
    { status: 404, bytes: 0 },
    { status: 200, bytes: -1 },
    { status: 200, bytes: Number.NaN },
    { status: 200, bytes: 1.5 },
  ]) {
    assert.throws(() => validateTileResponseSnapshot([response]), /invalid content length|unsuccessful/);
  }
});

test("atomic JSON publication cleans a failed temporary part", async () => {
  const directory = await mkdtemp(join(tmpdir(), "isophilly-visual-test-"));
  try {
    const target = join(directory, "current.json");
    await writeJsonAtomic(target, { success: true });
    assert.deepEqual(JSON.parse(await readFile(target, "utf8")), { success: true });

    const directoryTarget = join(directory, "cannot-replace");
    await mkdir(directoryTarget);
    await assert.rejects(writeJsonAtomic(directoryTarget, { success: false }));
    assert.deepEqual(
      (await readdir(directory)).filter((name) => name.endsWith(".part")),
      [],
    );
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("success publication preserves the pointer on report failure and links its hash", async () => {
  const directory = await mkdtemp(join(tmpdir(), "isophilly-publication-test-"));
  try {
    const reportPath = join(directory, "report.json");
    const currentPath = join(directory, "current.json");
    await writeJsonAtomic(currentPath, { runId: "previous" });
    const circular = {};
    circular.self = circular;
    await assert.rejects(
      publishSuccessEvidence({
        reportPath,
        currentPath,
        reportReference: "runs/failed/report.json",
        report: circular,
        current: { runId: "failed" },
      }),
    );
    assert.deepEqual(JSON.parse(await readFile(currentPath, "utf8")), { runId: "previous" });

    const pointer = await publishSuccessEvidence({
      reportPath,
      currentPath,
      reportReference: "runs/good/report.json",
      report: { success: true },
      current: { runId: "good" },
    });
    const reportBytes = await readFile(reportPath);
    const expectedHash = createHash("sha256").update(reportBytes).digest("hex");
    assert.equal(pointer.reportSha256, expectedHash);
    assert.deepEqual(JSON.parse(await readFile(currentPath, "utf8")), pointer);
    assert.equal(pointer.report, "runs/good/report.json");
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("teardown failure preserves the previous current-success pointer", async () => {
  const directory = await mkdtemp(join(tmpdir(), "isophilly-teardown-test-"));
  try {
    const reportPath = join(directory, "report.json");
    const currentPath = join(directory, "current.json");
    await writeJsonAtomic(currentPath, { runId: "previous" });
    let browserClosed = false;
    await assert.rejects(
      publishSuccessAfterTeardown(
        async () => {
          browserClosed = true;
        },
        async () => {
          throw new Error("server survived SIGKILL");
        },
        {
          reportPath,
          currentPath,
          reportReference: "runs/new/report.json",
          report: { success: true },
          current: { runId: "new" },
        },
      ),
      /survived SIGKILL/,
    );
    assert.equal(browserClosed, true);
    assert.equal(await readFile(reportPath, "utf8").catch(() => null), null);
    assert.deepEqual(JSON.parse(await readFile(currentPath, "utf8")), { runId: "previous" });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("city sector targets require every permanent outer-city smoke view", () => {
  const complete = [
    { sector: "far-northeast" },
    { sector: "north" },
    { sector: "southwest" },
    { sector: "lower-south" },
  ];
  assert.deepEqual(validateCitySectorTargets(complete), [
    "far-northeast",
    "lower-south",
    "north",
    "southwest",
  ]);
  assert.throws(
    () => validateCitySectorTargets(complete.slice(0, -1)),
    /missing: lower-south/,
  );
});

test("PNG evidence records exact bytes and dimensions", () => {
  const png = Buffer.alloc(24);
  Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]).copy(png);
  png.writeUInt32BE(1440, 16);
  png.writeUInt32BE(960, 20);
  assert.deepEqual(pngEvidence(png), {
    sha256: createHash("sha256").update(png).digest("hex"),
    bytes: 24,
    width: 1440,
    height: 960,
  });
  assert.throws(() => pngEvidence(Buffer.alloc(24)), /not a PNG/);
  png.writeUInt32BE(0, 16);
  assert.throws(() => pngEvidence(png), /invalid dimensions/);
});

test("run identity components cannot create paths", () => {
  assert.equal(
    safeRunComponent("2026-08-31T12:34:56.000Z feature/name"),
    "2026-08-31T12-34-56.000Z-feature-name",
  );
  assert.equal(safeRunComponent("../dirty\\tree"), "..-dirty-tree");
});
