import { test } from "node:test";
import assert from "node:assert/strict";

// ---------------------------------------------------------------------------
// Inline the client under test (avoids needing an npm install in CI).
// We re-import via a relative path so the test is self-contained.
// ---------------------------------------------------------------------------

import { SomaClient } from "./index.js";

// ---------------------------------------------------------------------------
// Sample ResultSet fixture
// ---------------------------------------------------------------------------

const SAMPLE_RAW = {
  columns: [
    { name: "service",      data_type: "string" },
    { name: "events.count", data_type: "number" },
    { name: "events.p99",   data_type: "number" },
  ],
  rows: [
    ["web",    42, 120],
    ["mobile", 17,  95],
    ["cli",     5, 200],
  ],
};

// ---------------------------------------------------------------------------
// Stub fetch for SomaClient tests
// ---------------------------------------------------------------------------

function makeClient(responseBody) {
  const stubFetch = async () => ({
    ok: true,
    status: 200,
    statusText: "OK",
    json: async () => responseBody,
  });

  // Patch global fetch locally.
  const origFetch = globalThis.fetch;
  globalThis.fetch = stubFetch;
  const client = new SomaClient(
    async () => "tok_test",
    { apiUrl: "http://localhost:4000" }
  );
  return { client, restore: () => { globalThis.fetch = origFetch; } };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test("tableData returns column names and raw rows", async () => {
  const { client, restore } = makeClient(SAMPLE_RAW);
  try {
    const rs = await client.query({ cube: "events", measures: ["events.count"], dimensions: ["events.service"] });
    const { columns, rows } = rs.tableData();
    assert.deepEqual(columns, ["service", "events.count", "events.p99"]);
    assert.equal(rows.length, 3);
    assert.deepEqual(rows[0], ["web", 42, 120]);
  } finally {
    restore();
  }
});

test("series infers first string column as X and number columns as measures", async () => {
  const { client, restore } = makeClient(SAMPLE_RAW);
  try {
    const rs = await client.query({ cube: "events", measures: ["events.count"], dimensions: ["events.service"] });
    const s = rs.series();
    assert.equal(s.length, 2, "two number columns → two series");

    assert.equal(s[0].name, "events.count");
    assert.deepEqual(s[0].points, [
      { x: "web",    y: 42 },
      { x: "mobile", y: 17 },
      { x: "cli",    y: 5 },
    ]);

    assert.equal(s[1].name, "events.p99");
    assert.deepEqual(s[1].points, [
      { x: "web",    y: 120 },
      { x: "mobile", y: 95 },
      { x: "cli",    y: 200 },
    ]);
  } finally {
    restore();
  }
});

test("series falls back to column 0 when no category column exists", async () => {
  const raw = {
    columns: [
      { name: "count", data_type: "number" },
      { name: "p99",   data_type: "number" },
    ],
    rows: [[10, 55]],
  };
  const { client, restore } = makeClient(raw);
  try {
    const rs = await client.query({});
    const s = rs.series();
    // Only the second column is a measure (col 0 is used as X, which is also number — edge case).
    // Both are number → catIdx = -1 → fallback 0, measures = both cols.
    // Points for col 1:
    assert.equal(s.length, 2);
    // x is stringified value of col 0 row cell
    assert.equal(s[1].points[0].x, "10");
    assert.equal(s[1].points[0].y, 55);
  } finally {
    restore();
  }
});

test("query throws on non-ok response", async () => {
  const origFetch = globalThis.fetch;
  globalThis.fetch = async () => ({ ok: false, status: 403, statusText: "Forbidden" });
  const client = new SomaClient(async () => "tok", { apiUrl: "http://localhost:4000" });
  try {
    await assert.rejects(
      () => client.query({}),
      /403/
    );
  } finally {
    globalThis.fetch = origFetch;
  }
});
