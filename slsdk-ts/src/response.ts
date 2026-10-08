/**
 * SDK response wrapper: keeps the underlying stream unconsumed by default;
 * callers choose json/text/bytes for one-shot reads, or stream for incremental processing.
 */
export class SdkResponse {
  constructor(private readonly raw: Response) {}

  get status(): number {
    return this.raw.status;
  }

  get headers(): Headers {
    return this.raw.headers;
  }

  /** The raw underlying byte stream; process incrementally for large files/streaming, never load it whole into memory */
  get stream(): ReadableStream<Uint8Array> {
    if (!this.raw.body) {
      throw new Error('response body has no readable stream (e.g. a 204/HEAD response, or the stream was already consumed)');
    }
    return this.raw.body;
  }

  json<T>(): Promise<T> {
    return this.raw.json();
  }

  text(): Promise<string> {
    return this.raw.text();
  }

  bytes(): Promise<Uint8Array> {
    return this.raw.arrayBuffer().then((buf) => new Uint8Array(buf));
  }
}
