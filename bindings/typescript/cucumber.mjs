// cucumber-js runs the shared conformance features (the same suite the Rust
// harness and every other binding run) against the TypeScript binding. Steps and
// fixtures are TypeScript, loaded via the tsx ESM loader (NODE_OPTIONS or
// --import tsx); the features are the canonical conformance/features tree plus
// the binding-local scenarios under conformance/features here, which exercise
// the TypeScript binding's own surface.
export default {
  paths: [
    "../../conformance/features/**/*.feature",
    "conformance/features/**/*.feature",
  ],
  import: ["conformance/**/*.ts"],
};
