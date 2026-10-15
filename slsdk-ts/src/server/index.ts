//! Public surface of `@superlive/sdk/server` (the Plane B server entry, the Node-family
//! sibling of slsdk-rs' `serve()`).

export { serve } from "./serve";
export type { ServeInfo } from "./serve-types";
export { ENV_ENDPOINT, ENV_TOKEN, EndpointParseError, parseEndpoint } from "./endpoint";
export type { Endpoint } from "./endpoint";
export { ServeError, formatEndpoint } from "./serve-error";
export type { ServeErrorKind } from "./serve-error";
export type { ServeHandler } from "./handler";
export type {
  ConfigField,
  ConfigFieldType,
  ComponentConfig,
  JsonValue,
  LaunchConfig,
  ServerManifest,
  TransportCaps,
  UiDeclaration,
  UiType,
} from "./manifest";
