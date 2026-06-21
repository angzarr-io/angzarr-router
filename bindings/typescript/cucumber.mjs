// cucumber-js runs the shared conformance features (the same suite the Rust
// harness and every other binding run) against the TypeScript binding. Steps and
// fixtures are TypeScript, loaded via the tsx ESM loader (NODE_OPTIONS or
// --import tsx); the features are the canonical conformance/features tree.
export default {
  paths: ["../../conformance/features/**/*.feature"],
  import: ["conformance/**/*.ts"],
};
