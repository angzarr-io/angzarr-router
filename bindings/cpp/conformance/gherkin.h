#pragma once

// A minimal Gherkin feature-runner: parses the shared conformance/features/*.feature
// (Scenario, Scenario Outline + Examples, Given/When/Then/And) and matches step
// text against a registry of cucumber-expression patterns ({int}, {string},
// {word}, optional "(s)"). There is no Reqnroll/cucumber-jvm for C++, so this is
// the "Catch2 feature-runner": each TEST_CASE loops a feature's scenarios under a
// DYNAMIC_SECTION and drives the steps; the step lambdas assert with Catch2.

#include <functional>
#include <regex>
#include <string>
#include <vector>

namespace angzarr::conformance {

struct Step {
  std::string text;
};

struct Scenario {
  std::string name;
  std::vector<Step> steps;
};

// Parses a feature file into scenarios, expanding each Scenario Outline over its
// Examples rows (<param> substituted from the row).
std::vector<Scenario> ParseFeature(const std::string& path);

// The absolute path to a shared feature file (ANGZARR_FEATURES_DIR is a compile
// definition pointing at the canonical conformance/features directory).
std::string FeaturePath(const std::string& name);

using StepArgs = std::vector<std::string>;
using StepFn = std::function<void(const StepArgs&)>;

// A per-scenario step registry. Patterns are cucumber expressions; the matched
// captures are passed to the step function as strings.
class StepRegistry {
 public:
  void On(const std::string& pattern, StepFn fn);
  // Matches text against the registered patterns and invokes the first match;
  // throws std::runtime_error when no pattern matches (a missing step).
  void Run(const std::string& text) const;

 private:
  struct Entry {
    std::regex regex;
    StepFn fn;
  };
  std::vector<Entry> entries_;
};

}  // namespace angzarr::conformance
