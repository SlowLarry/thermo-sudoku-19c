// Persistent, line-oriented CaDiCaL bridge for thermo-topology-cnf.
//
// This file contains no CaDiCaL source.  Build it against an independently
// obtained CaDiCaL library (CaDiCaL is MIT licensed).

#include "cadical.hpp"

#include <cctype>
#include <climits>
#include <cstdio>
#include <cstdint>
#include <cstdlib>
#include <iostream>
#include <limits>
#include <memory>
#include <sstream>
#include <string>
#include <unordered_set>
#include <vector>

#ifndef THERMO_CADICAL_REVISION
#define THERMO_CADICAL_REVISION unknown
#endif

#ifndef THERMO_CADICAL_LIBRARY_SHA256
#define THERMO_CADICAL_LIBRARY_SHA256 unknown
#endif

#define THERMO_STRINGIFY_INNER(token) #token
#define THERMO_STRINGIFY(token) THERMO_STRINGIFY_INNER (token)

namespace {

constexpr const char *PROTOCOL = "thermo-cadical-bridge-v1";
constexpr int EDGE_FIRST = 730;
constexpr int EDGE_LAST = 1273;
constexpr int OCCUPIED_FIRST = 1274;
constexpr int OCCUPIED_LAST = 1354;
constexpr int CADICAL_STRICT_DIMACS = 2;
constexpr std::size_t MAX_COMMAND_BYTES = 1u << 20;

struct Options {
  std::string cnf;
  int variables = -1;
  std::uint64_t clauses = std::numeric_limits<std::uint64_t>::max ();
  bool prefer_selected = false;
};

[[noreturn]] void usage_error (const std::string &message) {
  std::cerr << "cadical-incremental-bridge: " << message << '\n';
  std::cerr << "usage: cadical-incremental-bridge --cnf PATH --variables N "
               "--clauses N [--prefer-selected]\n";
  std::exit (2);
}

bool parse_u64 (const std::string &text, std::uint64_t &value) {
  if (text.empty ())
    return false;
  std::uint64_t result = 0;
  for (const unsigned char character : text) {
    if (!std::isdigit (character))
      return false;
    const unsigned digit = character - '0';
    if (result > (std::numeric_limits<std::uint64_t>::max () - digit) / 10)
      return false;
    result = result * 10 + digit;
  }
  value = result;
  return true;
}

bool parse_i32 (const std::string &text, int &value) {
  if (text.empty ())
    return false;
  bool negative = false;
  std::size_t offset = 0;
  if (text[0] == '-') {
    negative = true;
    offset = 1;
  } else if (text[0] == '+') {
    offset = 1;
  }
  if (offset == text.size ())
    return false;
  std::uint64_t magnitude = 0;
  if (!parse_u64 (text.substr (offset), magnitude))
    return false;
  const std::uint64_t maximum = negative
                                    ? std::uint64_t (INT_MAX) + 1
                                    : std::uint64_t (INT_MAX);
  if (magnitude > maximum)
    return false;
  if (negative && magnitude == std::uint64_t (INT_MAX) + 1)
    value = INT_MIN;
  else
    value = negative ? -static_cast<int> (magnitude)
                     : static_cast<int> (magnitude);
  return true;
}

Options parse_options (int argc, char **argv) {
  Options options;
  for (int index = 1; index < argc; ++index) {
    const std::string argument = argv[index];
    if (argument == "--prefer-selected") {
      options.prefer_selected = true;
      continue;
    }
    if (argument != "--cnf" && argument != "--variables" &&
        argument != "--clauses")
      usage_error ("unknown option '" + argument + "'");
    if (++index == argc)
      usage_error (argument + " requires a value");
    const std::string value = argv[index];
    if (argument == "--cnf") {
      if (!options.cnf.empty ())
        usage_error ("duplicate --cnf");
      options.cnf = value;
    } else if (argument == "--variables") {
      if (options.variables != -1)
        usage_error ("duplicate --variables");
      std::uint64_t parsed = 0;
      if (!parse_u64 (value, parsed) || parsed == 0 || parsed > INT_MAX)
        usage_error ("invalid --variables value");
      options.variables = static_cast<int> (parsed);
    } else {
      if (options.clauses != std::numeric_limits<std::uint64_t>::max ())
        usage_error ("duplicate --clauses");
      if (!parse_u64 (value, options.clauses))
        usage_error ("invalid --clauses value");
    }
  }
  if (options.cnf.empty () || options.variables < 1 ||
      options.clauses == std::numeric_limits<std::uint64_t>::max ())
    usage_error ("--cnf, --variables, and --clauses are required");
  return options;
}

// Read only far enough to cross-check the caller's expected DIMACS header from
// the same already-open handle later passed to CaDiCaL.  This avoids reopening
// the path between the precheck and the full parse.  CaDiCaL's strict=2 mode is
// the single full-body parser: it additionally validates canonical header
// whitespace, literal bounds, clause termination, and the exact clause count.
// Keeping a second token-by-token body pass here more than doubled startup I/O
// and its integer conversions dominated startup for large masters.
bool read_dimacs_header (FILE *input, std::uint64_t &variables,
                         std::uint64_t &clauses, std::string &error) {
  std::string line;
  bool at_eof = false;
  while (!at_eof) {
    line.clear ();
    for (;;) {
      const int character = std::fgetc (input);
      if (character == EOF) {
        if (std::ferror (input)) {
          error = "cannot read CNF";
          return false;
        }
        at_eof = true;
        break;
      }
      if (character == '\n')
        break;
      line.push_back (static_cast<char> (character));
    }
    if (line.empty () && at_eof)
      break;
    if (!line.empty () && line.back () == '\r')
      line.pop_back ();
    const std::size_t first = line.find_first_not_of (" \t");
    if (first == std::string::npos || line[first] == 'c')
      continue;
    std::istringstream parser (line);
    std::string p, cnf, variables_text, clauses_text, extra;
    if (!(parser >> p >> cnf >> variables_text >> clauses_text) ||
        (parser >> extra) || p != "p" || cnf != "cnf" ||
        !parse_u64 (variables_text, variables) ||
        !parse_u64 (clauses_text, clauses)) {
      error = "malformed DIMACS header";
      return false;
    }
    if (variables == 0 || variables > INT_MAX) {
      error = "DIMACS variable count is out of range";
      return false;
    }
    return true;
  }
  error = "missing DIMACS header";
  return false;
}

std::vector<std::string> split (const std::string &line) {
  std::istringstream parser (line);
  std::vector<std::string> tokens;
  std::string token;
  while (parser >> token)
    tokens.push_back (token);
  return tokens;
}

std::string metadata_token (const char *text) {
  std::string result;
  if (!text)
    return "unknown";
  for (const unsigned char character : std::string (text)) {
    if (std::isalnum (character) || character == '.' || character == '-' ||
        character == '_' || character == '+')
      result += static_cast<char> (character);
    else
      result += '_';
  }
  return result.empty () ? "unknown" : result;
}

[[noreturn]] void protocol_error (const std::string &message) {
  std::cout << "ERROR " << message << '\n' << std::flush;
  std::cerr << "cadical-incremental-bridge: protocol error: " << message
            << '\n';
  std::exit (2);
}

} // namespace

int main (int argc, char **argv) {
  const Options options = parse_options (argc, argv);

  using FileHandle = std::unique_ptr<FILE, decltype (&std::fclose)>;
  FileHandle cnf_file (std::fopen (options.cnf.c_str (), "rb"), &std::fclose);
  if (!cnf_file)
    usage_error ("cannot open CNF");

  std::uint64_t header_variables = 0, header_clauses = 0;
  std::string header_error;
  if (!read_dimacs_header (cnf_file.get (), header_variables, header_clauses,
                           header_error))
    usage_error (header_error);
  if (header_variables != static_cast<std::uint64_t> (options.variables) ||
      header_clauses != options.clauses)
    usage_error ("DIMACS header disagrees with --variables/--clauses");
  if (std::fseek (cnf_file.get (), 0, SEEK_SET))
    usage_error ("cannot rewind CNF");
  std::clearerr (cnf_file.get ());

  CaDiCaL::Solver solver;
  if (!solver.set ("quiet", 1))
    usage_error ("CaDiCaL rejected quiet option");
  int parsed_variables = 0;
  const char *parse_error_pointer = solver.read_dimacs (
      cnf_file.get (), options.cnf.c_str (), parsed_variables,
      CADICAL_STRICT_DIMACS);
  const std::string parse_error = parse_error_pointer ? parse_error_pointer : "";
  cnf_file.reset ();
  if (!parse_error.empty ())
    usage_error (std::string ("cannot parse CNF: ") + parse_error);
  if (parsed_variables != options.variables)
    usage_error ("CaDiCaL parsed an unexpected variable count");

  if (options.prefer_selected) {
    if (options.variables < OCCUPIED_LAST)
      usage_error ("--prefer-selected requires the topology variable ranges");
    for (int variable = EDGE_FIRST; variable <= EDGE_LAST; ++variable)
      solver.phase (variable);
    for (int variable = OCCUPIED_FIRST; variable <= OCCUPIED_LAST; ++variable)
      solver.phase (variable);
  }

  std::uint64_t added_clauses = 0;
  std::cout << "READY " << PROTOCOL << " variables=" << options.variables
            << " clauses=" << options.clauses
            << " cadical=" << metadata_token (CaDiCaL::Solver::signature ())
            << " revision="
            << metadata_token (THERMO_STRINGIFY (THERMO_CADICAL_REVISION))
            << " library_sha256="
            << metadata_token (
                   THERMO_STRINGIFY (THERMO_CADICAL_LIBRARY_SHA256))
            << " prefer_selected=" << (options.prefer_selected ? 1 : 0) << '\n'
            << std::flush;

  std::string line;
  while (std::getline (std::cin, line)) {
    if (!line.empty () && line.back () == '\r')
      line.pop_back ();
    if (line.size () > MAX_COMMAND_BYTES)
      protocol_error ("command_too_long");
    const std::vector<std::string> tokens = split (line);
    if (tokens.empty ())
      protocol_error ("empty_command");

    if (tokens[0] == "SOLVE") {
      if (tokens.size () != 2)
        protocol_error ("malformed_SOLVE");
      int conflicts = 0;
      if (!parse_i32 (tokens[1], conflicts) || conflicts < -1)
        protocol_error ("invalid_conflict_limit");
      if (!solver.limit ("conflicts", conflicts))
        protocol_error ("CaDiCaL_rejected_conflict_limit");
      const int status = solver.solve ();
      if (status == CaDiCaL::SATISFIABLE) {
        std::vector<int> model;
        model.reserve (options.variables);
        for (int variable = 1; variable <= options.variables; ++variable) {
          const int value = solver.val (variable);
          if (value != variable && value != -variable)
            protocol_error ("CaDiCaL_returned_invalid_model_value");
          model.push_back (value);
        }
        std::cout << "RESULT SAT " << options.variables << '\n' << "MODEL";
        for (const int literal : model)
          std::cout << ' ' << literal;
        std::cout << " 0\n" << std::flush;
      } else if (status == CaDiCaL::UNSATISFIABLE) {
        std::cout << "RESULT UNSAT\n" << std::flush;
      } else if (status == CaDiCaL::UNKNOWN) {
        std::cout << "RESULT UNKNOWN\n" << std::flush;
      } else {
        protocol_error ("CaDiCaL_returned_invalid_status");
      }
      continue;
    }

    if (tokens[0] == "ADD") {
      if (tokens.size () < 3)
        protocol_error ("malformed_ADD");
      std::uint64_t declared_size = 0;
      if (!parse_u64 (tokens[1], declared_size) ||
          declared_size > static_cast<std::uint64_t> (options.variables) ||
          tokens.size () != declared_size + 3 || tokens.back () != "0")
        protocol_error ("invalid_ADD_size");
      std::unordered_set<int> seen;
      std::vector<int> clause;
      clause.reserve (static_cast<std::size_t> (declared_size));
      for (std::size_t index = 0; index < declared_size; ++index) {
        int literal = 0;
        if (!parse_i32 (tokens[index + 2], literal) || literal == 0 ||
            literal == INT_MIN || std::abs (literal) > options.variables)
          protocol_error ("invalid_ADD_literal");
        if (!seen.insert (literal).second || seen.count (-literal))
          protocol_error ("duplicate_or_tautological_ADD_literal");
        clause.push_back (literal);
      }
      for (const int literal : clause)
        solver.add (literal);
      solver.add (0);
      ++added_clauses;
      std::cout << "ADDED " << added_clauses << ' ' << clause.size () << ' '
                << (options.clauses + added_clauses) << '\n'
                << std::flush;
      continue;
    }

    if (tokens[0] == "PING") {
      if (tokens.size () != 1)
        protocol_error ("malformed_PING");
      std::cout << "PONG " << added_clauses << ' '
                << (options.clauses + added_clauses) << '\n'
                << std::flush;
      continue;
    }

    if (tokens[0] == "QUIT") {
      if (tokens.size () != 1)
        protocol_error ("malformed_QUIT");
      std::cout << "BYE " << added_clauses << '\n' << std::flush;
      return 0;
    }

    protocol_error ("unknown_command");
  }
  // EOF is a normal cleanup path: if the Rust parent exits unexpectedly its
  // pipe closes and this process releases the solver instead of becoming an
  // orphan.
  return 0;
}
