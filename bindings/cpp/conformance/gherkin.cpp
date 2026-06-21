#include "gherkin.h"

#include <fstream>
#include <sstream>
#include <stdexcept>

namespace angzarr::conformance {
namespace {

std::string Trim(const std::string& s) {
  const auto b = s.find_first_not_of(" \t\r\n");
  if (b == std::string::npos) return "";
  const auto e = s.find_last_not_of(" \t\r\n");
  return s.substr(b, e - b + 1);
}

// Returns the step text after a leading Gherkin keyword, or empty if the line is
// not a step.
std::string StripKeyword(const std::string& line) {
  for (const char* kw : {"Given ", "When ", "Then ", "And ", "But ", "* "}) {
    const std::string k = kw;
    if (line.rfind(k, 0) == 0) return Trim(line.substr(k.size()));
  }
  return "";
}

std::vector<std::string> TableCells(const std::string& line) {
  std::vector<std::string> cells;
  std::stringstream ss(line);
  std::string cell;
  while (std::getline(ss, cell, '|')) cells.push_back(Trim(cell));
  // Drop the empty leading/trailing cells around the outer pipes.
  if (!cells.empty() && cells.front().empty()) cells.erase(cells.begin());
  if (!cells.empty() && cells.back().empty()) cells.pop_back();
  return cells;
}

// Converts a cucumber expression to an anchored regex, capturing {int}/{string}/
// {word} and treating "(s)" as an optional trailing s.
std::regex ToRegex(const std::string& pattern) {
  std::string rx = "^";
  for (size_t i = 0; i < pattern.size();) {
    if (pattern.compare(i, 5, "{int}") == 0) {
      rx += "(-?\\d+)";
      i += 5;
    } else if (pattern.compare(i, 8, "{string}") == 0) {
      rx += "\"([^\"]*)\"";
      i += 8;
    } else if (pattern.compare(i, 6, "{word}") == 0) {
      rx += "(\\S+)";
      i += 6;
    } else if (pattern.compare(i, 3, "(s)") == 0) {
      rx += "s?";
      i += 3;
    } else {
      const char c = pattern[i];
      if (std::string(".^$|()[]{}*+?\\/").find(c) != std::string::npos) rx += '\\';
      rx += c;
      ++i;
    }
  }
  rx += "$";
  return std::regex(rx);
}

}  // namespace

std::vector<Scenario> ParseFeature(const std::string& path) {
  std::ifstream in(path);
  if (!in) throw std::runtime_error("cannot open feature: " + path);

  std::vector<Scenario> out;
  bool in_outline = false;
  bool in_examples = false;
  std::string outline_name;
  std::vector<std::string> outline_steps;
  std::vector<std::string> headers;
  Scenario* current = nullptr;

  std::string raw;
  while (std::getline(in, raw)) {
    const std::string line = Trim(raw);
    if (line.empty() || line[0] == '#' || line[0] == '@' || line.rfind("Feature:", 0) == 0) {
      continue;
    }
    if (line.rfind("Scenario Outline:", 0) == 0) {
      in_outline = true;
      in_examples = false;
      outline_name = Trim(line.substr(std::string("Scenario Outline:").size()));
      outline_steps.clear();
      headers.clear();
      current = nullptr;
      continue;
    }
    if (line.rfind("Scenario:", 0) == 0) {
      in_outline = false;
      in_examples = false;
      out.push_back({Trim(line.substr(std::string("Scenario:").size())), {}});
      current = &out.back();
      continue;
    }
    if (line.rfind("Examples:", 0) == 0) {
      in_examples = true;
      headers.clear();
      continue;
    }
    if (line[0] == '|') {
      const auto cells = TableCells(line);
      if (headers.empty()) {
        headers = cells;
      } else {
        Scenario sc;
        sc.name = outline_name + " [" + cells.front() + "]";
        for (const auto& tmpl : outline_steps) {
          std::string s = tmpl;
          for (size_t i = 0; i < headers.size() && i < cells.size(); ++i) {
            const std::string token = "<" + headers[i] + ">";
            for (auto pos = s.find(token); pos != std::string::npos; pos = s.find(token)) {
              s.replace(pos, token.size(), cells[i]);
            }
          }
          sc.steps.push_back({s});
        }
        out.push_back(std::move(sc));
      }
      continue;
    }
    const std::string text = StripKeyword(line);
    if (text.empty()) continue;  // feature/scenario prose, not a step
    if (in_outline && !in_examples) {
      outline_steps.push_back(text);
    } else if (current != nullptr) {
      current->steps.push_back({text});
    }
  }
  return out;
}

std::string FeaturePath(const std::string& name) {
  return std::string(ANGZARR_FEATURES_DIR) + "/" + name;
}

void StepRegistry::On(const std::string& pattern, StepFn fn) {
  entries_.push_back({ToRegex(pattern), std::move(fn)});
}

void StepRegistry::Run(const std::string& text) const {
  for (const auto& entry : entries_) {
    std::smatch m;
    if (std::regex_match(text, m, entry.regex)) {
      StepArgs args;
      for (size_t i = 1; i < m.size(); ++i) args.push_back(m[i].str());
      entry.fn(args);
      return;
    }
  }
  throw std::runtime_error("no step definition for: " + text);
}

}  // namespace angzarr::conformance
