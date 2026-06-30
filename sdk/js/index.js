/**
 * @soma-analytics/client
 *
 * Zero-dependency browser fetch wrapper for the soma-analytics query API.
 * The host supplies a `fetchToken` callback that returns a scoped bearer token;
 * the client calls it transparently before each request.
 */

// ---------------------------------------------------------------------------
// ResultSet wrapper
// ---------------------------------------------------------------------------

/**
 * Wraps the raw JSON response from POST /api/v1/query.
 *
 * Shape expected from the server:
 *   { columns: [{ name, data_type }], rows: any[][] }
 */
class ResultSet {
  /** @param {object} raw */
  constructor(raw) {
    /** The raw JSON object returned by the server. */
    this.raw = raw;
  }

  /**
   * Returns `{ columns: string[], rows: any[][] }` — column names + row data.
   * @returns {{ columns: string[], rows: any[][] }}
   */
  tableData() {
    const columns = (this.raw.columns ?? []).map((c) => c.name);
    const rows = this.raw.rows ?? [];
    return { columns, rows };
  }

  /**
   * Infers series from the result:
   *   - X axis = first column whose data_type is "string", "time", or "boolean"
   *     (falls back to column 0 if none found).
   *   - A series is created for each "number" column.
   *
   * @returns {Array<{ name: string, points: Array<{ x: string, y: number }> }>}
   */
  series() {
    const cols = this.raw.columns ?? [];
    const rows = this.raw.rows ?? [];

    // Locate the category (X) column index.
    const CATEGORY_TYPES = new Set(["string", "time", "boolean"]);
    let catIdx = cols.findIndex((c) => CATEGORY_TYPES.has(c.data_type));
    if (catIdx === -1) catIdx = 0;

    // Collect measure (number) column indices.
    const measureIdxs = cols
      .map((c, i) => ({ c, i }))
      .filter(({ c }) => c.data_type === "number")
      .map(({ i }) => i);

    return measureIdxs.map((mi) => ({
      name: cols[mi]?.name ?? `col_${mi}`,
      points: rows.map((row) => ({
        x: row[catIdx] == null ? "" : String(row[catIdx]),
        y: Number(row[mi] ?? 0),
      })),
    }));
  }
}

// ---------------------------------------------------------------------------
// SomaClient
// ---------------------------------------------------------------------------

export class SomaClient {
  /**
   * @param {() => Promise<string>} fetchToken
   *   Async callback the host implements to return a scoped bearer/embed token.
   *   Called transparently before each request.
   * @param {{ apiUrl: string }} options
   */
  constructor(fetchToken, { apiUrl }) {
    this._fetchToken = fetchToken;
    this._apiUrl = apiUrl.replace(/\/$/, ""); // strip trailing slash
  }

  /** @returns {Promise<string>} */
  async _token() {
    return this._fetchToken();
  }

  /**
   * POST /api/v1/query
   * @param {object} body  e.g. { cube, measures, dimensions, filters, limit }
   * @returns {Promise<ResultSet>}
   */
  async query(body) {
    const token = await this._token();
    const res = await fetch(`${this._apiUrl}/api/v1/query`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${token}`,
      },
      body: JSON.stringify(body),
    });
    if (!res.ok) {
      throw new Error(`soma-analytics query failed: ${res.status} ${res.statusText}`);
    }
    return new ResultSet(await res.json());
  }

  /**
   * GET /api/v1/meta
   * @returns {Promise<object>}
   */
  async meta() {
    const token = await this._token();
    const res = await fetch(`${this._apiUrl}/api/v1/meta`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    if (!res.ok) {
      throw new Error(`soma-analytics meta failed: ${res.status} ${res.statusText}`);
    }
    return res.json();
  }
}
