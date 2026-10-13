#include "shadervalues.h"

#include <cctype>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <map>
#include <sstream>

namespace {
    // The text of an effect file without its comments.
    std::string withoutComments(const std::string& text) {
        std::string out;
        out.reserve(text.size());
        size_t i = 0;
        while (i < text.size()) {
            if (text.compare(i, 2, "//") == 0) {
                while (i < text.size() && text[i] != '\n') {
                    ++i;
                }
            } else if (text.compare(i, 2, "/*") == 0) {
                size_t end = text.find("*/", i + 2);
                i = (end == std::string::npos) ? text.size() : end + 2;
                out += ' ';
            } else {
                out += text[i++];
            }
        }
        return out;
    }

    bool isNameChar(char c) {
        return std::isalnum(static_cast<unsigned char>(c)) || c == '_';
    }

    // The words and the signs of a statement. A number, a name and a sign are one token each.
    std::vector<std::string> tokens(const std::string& statement) {
        std::vector<std::string> out;
        size_t i = 0;
        while (i < statement.size()) {
            char c = statement[i];
            if (std::isspace(static_cast<unsigned char>(c))) {
                ++i;
            } else if (isNameChar(c) || (c == '.' && i + 1 < statement.size() && std::isdigit(static_cast<unsigned char>(statement[i + 1])))) {
                size_t start = i;
                while (i < statement.size() && (isNameChar(statement[i]) || statement[i] == '.')) {
                    // The sign of an exponent: 1e-6.
                    if ((statement[i] == 'e' || statement[i] == 'E') && i + 1 < statement.size() && (statement[i + 1] == '-' || statement[i + 1] == '+')
                        && std::isdigit(static_cast<unsigned char>(statement[start]))) {
                        ++i;
                    }
                    ++i;
                }
                out.push_back(statement.substr(start, i - start));
            } else {
                out.push_back(std::string(1, c));
                ++i;
            }
        }
        return out;
    }

    // A token that is a number: its value.
    bool number(const std::string& token, float* value) {
        if (token.empty() || !(std::isdigit(static_cast<unsigned char>(token[0])) || token[0] == '.')) {
            return false;
        }
        std::string text = token;
        while (!text.empty() && (text.back() == 'f' || text.back() == 'F')) {
            text.pop_back();
        }
        char* end = nullptr;
        *value = static_cast<float>(std::strtod(text.c_str(), &end));
        return end != text.c_str();
    }

    // The values of a type name for one element, and its letter for one element: 0 for a
    // type that is not a variable a mod can set (a texture, a sampler).
    char typeOf(const std::string& word, size_t* columns) {
        if (word == "float" || word == "float1" || word == "half" || word == "double") {
            *columns = 1;
            return 'f';
        }
        if (word == "int" || word == "uint" || word == "dword") {
            *columns = 1;
            return 'i';
        }
        if (word == "bool") {
            *columns = 1;
            return 'b';
        }
        if (word == "float2" || word == "float3" || word == "float4") {
            *columns = static_cast<size_t>(word[5] - '0');
            return word[5];
        }
        if (word == "float4x4" || word == "matrix") {
            *columns = 16;
            return 'm';
        }
        if (word == "string") {
            *columns = 0;
            return 's';
        }
        return 0;
    }
}

namespace {
    // Reads the variables of one file into `result`, and those of the files that it
    // includes. `constants` has the numbers that a size of an array can name: `#define
    // NAME 2` and `static const float NAME = 2;`.
    void parseFile(const std::string& path, ShaderValues& result, std::map<std::string, float>& constants, int depth);
}

ShaderValues ShaderValues::parse(const char* path) {
    ShaderValues result;
    std::map<std::string, float> constants;
    parseFile(path, result, constants, 0);
    return result;
}

namespace {
void parseFile(const std::string& path, ShaderValues& result, std::map<std::string, float>& constants, int includeDepth) {
    std::ifstream file(path, std::ios::binary);
    if (!file) {
        return;
    }
    std::stringstream buffer;
    buffer << file.rdbuf();
    const std::string text = withoutComments(buffer.str());

    // The statements outside of a block. A block (a function, a sampler state, a
    // technique) ends a statement without a variable in it.
    std::string statement;
    int depth = 0;
    size_t i = 0;
    while (i < text.size()) {
        char c = text[i];
        if (depth == 0 && c == '#') {
            // A line of the preprocessor.
            size_t end = text.find('\n', i);
            if (end == std::string::npos) {
                end = text.size();
            }
            const std::string line = text.substr(i + 1, end - i - 1);
            const auto words = tokens(line);
            float value;
            if (words.size() == 3 && words[0] == "define" && number(words[2], &value)) {
                constants[words[1]] = value;
            }
            // A file that this one includes, beside it: `#include "name.fx"`.
            const size_t open = line.find('"');
            const size_t close = line.rfind('"');
            if (!words.empty() && words[0] == "include" && open != std::string::npos && close > open && includeDepth < 8) {
                const size_t slash = path.find_last_of("\\/");
                const std::string folder = (slash == std::string::npos) ? std::string() : path.substr(0, slash + 1);
                parseFile(folder + line.substr(open + 1, close - open - 1), result, constants, includeDepth + 1);
            }
            i = end;
            continue;
        }
        const size_t last = statement.find_last_not_of(" \t\r\n");
        if (c == '{' && depth == 0 && last != std::string::npos && statement[last] == '=') {
            // The start value of a variable in braces, `float3 name = {1, 2, 3};`: a part
            // of the statement, not a block.
            int inner = 0;
            while (i < text.size()) {
                statement += text[i];
                if (text[i] == '{') {
                    ++inner;
                } else if (text[i] == '}' && --inner == 0) {
                    break;
                }
                ++i;
            }
        } else if (c == '{') {
            ++depth;
        } else if (c == '}') {
            if (depth > 0 && --depth == 0) {
                statement.clear();
            }
        } else if (depth == 0 && c == ';') {
            auto words = tokens(statement);
            statement.clear();
            ++i;

            // The words before the type.
            size_t at = 0;
            bool fixed = false;
            while (at < words.size() && (words[at] == "extern" || words[at] == "uniform" || words[at] == "shared" || words[at] == "static"
                                         || words[at] == "const" || words[at] == "volatile")) {
                fixed = fixed || words[at] == "static";
                ++at;
            }
            if (at >= words.size()) {
                continue;
            }
            size_t columns = 0;
            const char letter = typeOf(words[at], &columns);
            if (!letter) {
                continue;
            }
            ++at;

            // The names of the statement, with commas between them.
            while (at < words.size()) {
                if (!isNameChar(words[at][0]) || std::isdigit(static_cast<unsigned char>(words[at][0]))) {
                    break;
                }
                ShaderVariable variable;
                variable.name = words[at++];
                variable.set = false;

                // The sizes of an array: a number, or a name of a constant.
                size_t elements = 1;
                bool array = false;
                while (at + 2 < words.size() && words[at] == "[") {
                    float size = 0;
                    if (!number(words[at + 1], &size)) {
                        auto found = constants.find(words[at + 1]);
                        size = (found == constants.end()) ? 0.0f : found->second;
                    }
                    elements *= static_cast<size_t>(size > 0 ? size : 0);
                    array = true;
                    at += 3;
                }
                // A semantic.
                if (at + 1 < words.size() && words[at] == ":") {
                    at += 2;
                }
                // The start value: the numbers up to the comma that ends the name.
                std::vector<float> start;
                if (at < words.size() && words[at] == "=") {
                    ++at;
                    int inner = 0;
                    while (at < words.size()) {
                        const std::string& word = words[at];
                        if (word == "(" || word == "{") {
                            ++inner;
                        } else if (word == ")" || word == "}") {
                            --inner;
                        } else if (word == "," && inner == 0) {
                            break;
                        } else {
                            float value;
                            if (number(word, &value)) {
                                // A sign before the number.
                                if (at > 0 && words[at - 1] == "-") {
                                    value = -value;
                                }
                                start.push_back(value);
                            } else if (word == "true") {
                                start.push_back(1.0f);
                            } else if (word == "false") {
                                start.push_back(0.0f);
                            }
                        }
                        ++at;
                    }
                }
                if (at < words.size() && words[at] == ",") {
                    ++at;
                }

                if (fixed) {
                    // A value of the file itself, not a variable: a size can name it.
                    if (!array && columns == 1 && !start.empty()) {
                        constants[variable.name] = start[0];
                    }
                    continue;
                }

                if (letter == 's') {
                    variable.type = 's';
                } else if (!array) {
                    variable.type = letter;
                    variable.values.assign(columns, 0.0f);
                } else if (columns == 1) {
                    variable.type = static_cast<char>(std::toupper(static_cast<unsigned char>(letter)));
                    variable.values.assign(elements, 0.0f);
                } else if (letter == '2' || letter == '3' || letter == '4') {
                    // An array of vectors has 4 values for each vector in the API.
                    variable.type = 'V';
                    variable.values.assign(elements * 4, 0.0f);
                } else {
                    // An array of matrices: not in the API.
                    continue;
                }
                for (size_t n = 0; n < start.size() && n < variable.values.size(); ++n) {
                    variable.values[n] = start[n];
                }
                // A name comes one time: a file and a file that it includes can have it.
                if (!result.find(variable.name.c_str())) {
                    result.variables.push_back(variable);
                }
            }
            continue;
        } else if (depth == 0) {
            statement += c;
        }
        ++i;
    }
}
}

ShaderVariable* ShaderValues::find(const char* name) {
    for (auto& variable : variables) {
        if (variable.name == name) {
            return &variable;
        }
    }
    return nullptr;
}

bool ShaderValues::set(const char* name, const float* values, size_t count) {
    ShaderVariable* variable = find(name);
    if (!variable || variable->type == 's') {
        return false;
    }
    for (size_t n = 0; n < count && n < variable->values.size(); ++n) {
        variable->values[n] = values[n];
    }
    variable->set = true;
    changed = true;
    return true;
}

size_t ShaderValues::get(const char* name, float* out, size_t count) {
    ShaderVariable* variable = find(name);
    if (!variable) {
        return 0;
    }
    for (size_t n = 0; n < count && n < variable->values.size(); ++n) {
        out[n] = variable->values[n];
    }
    return variable->values.size();
}
