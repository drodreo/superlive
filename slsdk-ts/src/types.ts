/**
 * Client configuration for the Plane A protocol (the shell HTTP gateway).
 * baseUrl is expected to include the full /api prefix (e.g. http://127.0.0.1:39876/api);
 * the SDK applies zero prefix processing to paths, pure concatenation; a trailing slash on baseUrl is normalized away at creation time,
 * so behavior is identical with or without a trailing slash.
 */
export interface ClientOptions {
  baseUrl: string;
  token: string;
}

/**
 * Request body types that can be sent. File is a subtype of Blob and is covered naturally;
 * objects/arrays/primitives all go through the JSON wire format (the wire is UTF-8 bytes anyway),
 * binary/stream/form types are passed through as-is, never base64 or similar encodings.
 */
export type RequestBody =
  | Record<string, unknown>
  | unknown[]
  | string
  | number
  | boolean
  | null
  | Blob
  | Uint8Array
  | ArrayBuffer
  | ReadableStream<Uint8Array>
  | FormData;

export interface RequestOptions {
  /** Query parameters appended to the URL */
  query?: Record<string, string | number | boolean>;
  /** Extra request headers, written last; may override the SDK-generated Content-Type and Authorization */
  headers?: Record<string, string>;
  /** Cancellation signal, passed through to the underlying fetch */
  signal?: AbortSignal;
}
