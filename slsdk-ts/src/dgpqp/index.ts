// dgpqp: drodreo's general purpose query primitive
// ts + rs maintain the same query semantics across languages (the component-private copies were consolidated into the @shell/sdk api entry).
// Consumers import directly from '@shell/sdk' (import { QuerySchema, ... } from '@shell/sdk').

export * from './schema';
export * from './condition';
export * from './operator';
export * from './sorting';
export * from './pagination';
