import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { rename, unlink, writeFile } from "node:fs/promises";

let atomicWriteSequence = 0;

export const REQUIRED_CITY_SECTORS = ["far-northeast", "north", "southwest", "lower-south"];
export const CANONICAL_VISUAL_ZOOMS = Object.freeze([3, 4, 5, 7, 9, 10]);

/** @param {string | undefined} raw @param {boolean} fallback @param {string} name */
export function binaryFlagSetting(raw, fallback, name) {
  if (raw === undefined) return fallback;
  if (raw === "0") return false;
  if (raw === "1") return true;
  throw new Error(`invalid ${name}: ${raw}; expected 0 or 1`);
}

/** @param {string | undefined} raw @param {number} fallback @param {string} name @param {number} minimum @param {number} maximum */
export function integerSetting(raw, fallback, name, minimum, maximum) {
  if (raw === undefined) return fallback;
  if (!/^\d+$/.test(raw)) throw new Error(`invalid ${name}: ${raw}`);
  const value = Number(raw);
  if (!Number.isSafeInteger(value) || value < minimum || value > maximum) {
    throw new Error(`invalid ${name}: ${raw}`);
  }
  return value;
}

/** @param {string | undefined} raw @param {readonly number[]} fallback @param {string} name @param {number} minimum @param {number} maximum */
export function integerListSetting(raw, fallback, name, minimum, maximum) {
  if (raw === undefined) return [...fallback];
  if (!/^\d+(,\d+)*$/.test(raw)) throw new Error(`invalid ${name}: ${raw}`);
  const values = raw.split(",").map(Number);
  if (
    values.some(
      (value) => !Number.isSafeInteger(value) || value < minimum || value > maximum,
    )
  ) {
    throw new Error(`invalid ${name}: ${raw}`);
  }
  if (new Set(values).size !== values.length) throw new Error(`invalid ${name}: ${raw}`);
  return values;
}

/** @param {number[]} zooms @param {boolean} releaseMode */
export function validateReleaseZooms(zooms, releaseMode) {
  if (
    releaseMode &&
    (zooms.length !== CANONICAL_VISUAL_ZOOMS.length ||
      zooms.some((zoom, index) => zoom !== CANONICAL_VISUAL_ZOOMS[index]))
  ) {
    throw new Error(
      `release visual QA requires canonical zooms: ${CANONICAL_VISUAL_ZOOMS.join(",")}`,
    );
  }
}

/** @param {Promise<unknown>[]} tasks */
export async function drainGrowingTasks(tasks) {
  let drained = 0;
  while (true) {
    const target = tasks.length;
    await Promise.all(tasks.slice(drained, target));
    drained = target;
    await new Promise((resolve) => setImmediate(resolve));
    if (tasks.length === drained) return;
  }
}

/** @param {{ exitCode: number | null, signalCode: string | null }} child */
export function childHasExited(child) {
  return child.exitCode !== null || child.signalCode !== null;
}

/**
 * @param {{ exitCode: number | null, signalCode: string | null, kill: (signal: string) => boolean }} child
 * @param {(timeout: number) => Promise<boolean>} waitForExit
 * @param {number} timeout
 */
export async function stopChild(child, waitForExit, timeout = 3_000) {
  if (childHasExited(child)) return;
  child.kill("SIGTERM");
  if (await waitForExit(timeout)) return;
  child.kill("SIGKILL");
  if (!(await waitForExit(timeout))) {
    throw new Error("server did not exit after SIGKILL");
  }
}

/** @template {Record<string, unknown>} T @param {T[]} records */
export function freezeRecords(records) {
  return Object.freeze(records.map((record) => Object.freeze({ ...record })));
}

/** @param {readonly { status: number, bytes: number }[]} responses */
export function validateTileResponseSnapshot(responses) {
  if (responses.length === 0) throw new Error("view recorded no tile responses");
  if (
    responses.some(
      (response) =>
        !Number.isInteger(response.status) ||
        response.status < 200 ||
        response.status >= 300 ||
        !Number.isSafeInteger(response.bytes) ||
        response.bytes < 0,
    )
  ) {
    throw new Error("view recorded an unsuccessful tile response or invalid content length");
  }
}

/** @param {string} path */
export async function sha256File(path) {
  const digest = createHash("sha256");
  for await (const chunk of createReadStream(path)) digest.update(chunk);
  return digest.digest("hex");
}

/** @param {string} path @param {unknown} value */
export async function writeJsonAtomic(path, value) {
  atomicWriteSequence += 1;
  const temporary = `${path}.${process.pid}-${atomicWriteSequence}.part`;
  try {
    await writeFile(temporary, `${JSON.stringify(value, null, 2)}\n`);
    await rename(temporary, path);
  } catch (error) {
    await unlink(temporary).catch(() => {});
    throw error;
  }
}

/**
 * @param {{ reportPath: string, currentPath: string, reportReference: string, report: unknown, current: Record<string, unknown> }} publication
 */
export async function publishSuccessEvidence(publication) {
  await writeJsonAtomic(publication.reportPath, publication.report);
  const reportSha256 = await sha256File(publication.reportPath);
  const current = {
    ...publication.current,
    report: publication.reportReference,
    reportSha256,
  };
  await writeJsonAtomic(publication.currentPath, current);
  return current;
}

/**
 * @param {() => Promise<void>} closeBrowser
 * @param {() => Promise<void>} stopServer
 * @param {{ reportPath: string, currentPath: string, reportReference: string, report: unknown, current: Record<string, unknown> }} publication
 */
export async function publishSuccessAfterTeardown(closeBrowser, stopServer, publication) {
  let closeError;
  try {
    await closeBrowser();
  } catch (error) {
    closeError = error;
  }
  try {
    await stopServer();
  } catch (stopError) {
    if (closeError !== undefined) {
      throw new AggregateError([closeError, stopError], "browser and server teardown failed");
    }
    throw stopError;
  }
  if (closeError !== undefined) throw closeError;
  return publishSuccessEvidence(publication);
}

/** @param {{ sector?: string }[]} targets */
export function validateCitySectorTargets(targets) {
  const sectors = new Set(targets.map((target) => target.sector).filter(Boolean));
  const missing = REQUIRED_CITY_SECTORS.filter((sector) => !sectors.has(sector));
  if (missing.length > 0) throw new Error(`visual target matrix is missing: ${missing.join(", ")}`);
  return [...sectors].sort();
}

/** @param {Buffer} bytes */
export function pngEvidence(bytes) {
  const signature = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
  if (bytes.length < 24 || !bytes.subarray(0, 8).equals(signature)) {
    throw new Error("Playwright screenshot is not a PNG");
  }
  const width = bytes.readUInt32BE(16);
  const height = bytes.readUInt32BE(20);
  if (width < 1 || height < 1) throw new Error("Playwright screenshot has invalid dimensions");
  return {
    sha256: createHash("sha256").update(bytes).digest("hex"),
    bytes: bytes.length,
    width,
    height,
  };
}

/** @param {string} value */
export function safeRunComponent(value) {
  return value.replaceAll(/[^a-zA-Z0-9._-]/g, "-").replaceAll(/-+/g, "-");
}
