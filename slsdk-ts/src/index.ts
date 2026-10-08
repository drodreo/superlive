export {
  createClient,
  type CreateClientConfig,
  type HttpMethod,
  type SdkClient,
} from './client';
export { ApiError } from './errors';
export { SdkResponse } from './response';
export type { ClientOptions, RequestBody, RequestOptions } from './types';
export type { ObserveOptions, RetryOptions } from './observe';

// legacy query primitives (the filter types double as API request bodies, aligned with the component-side legacy cross-language protocol)
export * from './legacy';
