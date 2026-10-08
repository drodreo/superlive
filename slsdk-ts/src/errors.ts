/** The error thrown uniformly for non-2xx responses; body is the best-effort parsed response body (JSON object or raw text) */
export class ApiError extends Error {
  readonly status: number;
  readonly body: unknown;
  readonly raw?: Response;

  constructor(message: string, status: number, body: unknown, raw?: Response) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.body = body;
    this.raw = raw;
  }

  /** Builds the error from a non-2xx Response; clones an independent stream for parsing, leaving the original body unconsumed */
  static async from(res: Response): Promise<ApiError> {
    let body: unknown;
    let summary = '';
    try {
      body = await res.clone().json();
      summary = JSON.stringify(body) ?? '';
    } catch {
      try {
        const text = await res.clone().text();
        body = text;
        summary = text;
      } catch {
        body = undefined;
      }
    }
    const message = summary
      ? `API ${res.status}: ${summary.slice(0, 200)}`
      : `API ${res.status}`;
    return new ApiError(message, res.status, body, res);
  }
}
