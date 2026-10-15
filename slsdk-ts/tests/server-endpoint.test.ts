import { describe, expect, it } from 'vitest';
import { EndpointParseError, parseEndpoint } from '../src/server/endpoint';

describe('parseEndpoint', () => {
  it('uds: takes the path verbatim', () => {
    expect(parseEndpoint('uds:/tmp/slsdk-test.sock')).toEqual({
      kind: 'uds',
      path: '/tmp/slsdk-test.sock',
    });
  });

  it('uds: alone yields an empty path (rs parity: strip_prefix then to_string)', () => {
    expect(parseEndpoint('uds:')).toEqual({ kind: 'uds', path: '' });
  });

  it('tcp: ipv4 host:port parses; port 0 (random port form) is valid', () => {
    expect(parseEndpoint('tcp:127.0.0.1:8080')).toEqual({ kind: 'tcp', host: '127.0.0.1', port: 8080 });
    expect(parseEndpoint('tcp:127.0.0.1:0')).toEqual({ kind: 'tcp', host: '127.0.0.1', port: 0 });
    expect(parseEndpoint('tcp:0.0.0.0:1')).toEqual({ kind: 'tcp', host: '0.0.0.0', port: 1 });
  });

  it('tcp: bracketed ipv6 parses (bare multi-colon ipv6 is rejected, SocketAddr grammar parity)', () => {
    expect(parseEndpoint('tcp:[::1]:8080')).toEqual({ kind: 'tcp', host: '::1', port: 8080 });
    expect(() => parseEndpoint('tcp:::1:8080')).toThrow(EndpointParseError);
  });

  it('missing scheme throws MissingScheme carrying the full value (message mirrors rs wording)', () => {
    const err = (() => {
      try {
        parseEndpoint('http://x');
      } catch (e) {
        return e as EndpointParseError;
      }
      return undefined;
    })();
    expect(err).toBeInstanceOf(EndpointParseError);
    expect(err?.kind).toBe('MissingScheme');
    expect(err?.value).toBe('http://x');
    expect(err?.message).toBe('endpoint is missing the uds:/tcp: prefix: http://x');
  });

  it.each([
    'tcp:', // rs: "".parse::<SocketAddr>() fails → InvalidTcpAddr
    'tcp:127.0.0.1', // no port
    'tcp:127.0.0.1:', // empty port
    'tcp:127.0.0.1:abc', // non-numeric port
    'tcp:127.0.0.1:99999', // port overflow (u16)
    'tcp::8080', // empty host (rs: ":8080" fails to parse)
    'tcp:[::1', // unclosed bracket
    'tcp:[::1]8080', // missing port separator after bracket
  ])('%s → InvalidTcpAddr with the full descriptor as value', (bad) => {
    let err: EndpointParseError | undefined;
    try {
      parseEndpoint(bad);
    } catch (e) {
      err = e as EndpointParseError;
    }
    expect(err, bad).toBeInstanceOf(EndpointParseError);
    expect(err?.kind, bad).toBe('InvalidTcpAddr');
    expect(err?.value, bad).toBe(bad);
    expect(err?.message, bad).toBe(`invalid tcp address in endpoint: ${bad}`);
  });
});
