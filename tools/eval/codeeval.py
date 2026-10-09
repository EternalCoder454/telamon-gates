#!/usr/bin/env python3
"""Coding test for a model on llama-server: 8 tasks, each in Rust, Go and
Python, with tests; the reply's code is compiled and run against them (so
run it in a throwaway container with rustc, go and python3). Prints passes
per language, time and tokens/s; the answers that failed are kept in
$EVAL_OUT/fail/<name>/.

Usage: codeeval.py <name> <llama-server args, including -m <model.gguf>>
Environment: THINK=1 lets the model reason first (Qwen3 thinking, gpt-oss
medium effort); see README.md for the rest."""
import os, re, shutil, subprocess, sys, tempfile

from common import chat, no_thinking, out_path, server

PORT = int(os.environ.get("EVAL_PORT", "18300"))
# THINK=1: the model reasons first (Qwen3 thinking, gpt-oss medium effort).
THINK = os.environ.get("THINK") == "1"

# (description, {lang: (signature, tests)})
TASKS = [
    ("checks whether a string is a palindrome, ignoring case and non-alphanumeric characters", {
        "python": ("def is_palindrome(s: str) -> bool",
                   "assert is_palindrome('A man, a plan, a canal: Panama')\nassert not is_palindrome('race a car')\nassert is_palindrome('')"),
        "rust": ("fn is_palindrome(s: &str) -> bool",
                 "assert!(is_palindrome(\"A man, a plan, a canal: Panama\"));\nassert!(!is_palindrome(\"race a car\"));\nassert!(is_palindrome(\"\"));"),
        "go": ("func IsPalindrome(s string) bool",
               "check(IsPalindrome(\"A man, a plan, a canal: Panama\"))\ncheck(!IsPalindrome(\"race a car\"))\ncheck(IsPalindrome(\"\"))"),
    }),
    ("merges overlapping intervals (inclusive ends; touching ones merge) and returns them sorted by start", {
        "python": ("def merge_intervals(xs: list[tuple[int, int]]) -> list[tuple[int, int]]",
                   "assert merge_intervals([(1,3),(2,6),(8,10),(15,18)])==[(1,6),(8,10),(15,18)]\nassert merge_intervals([(1,4),(4,5)])==[(1,5)]\nassert merge_intervals([])==[]"),
        "rust": ("fn merge_intervals(xs: Vec<(i64, i64)>) -> Vec<(i64, i64)>",
                 "assert_eq!(merge_intervals(vec![(1,3),(2,6),(8,10),(15,18)]), vec![(1,6),(8,10),(15,18)]);\nassert_eq!(merge_intervals(vec![(1,4),(4,5)]), vec![(1,5)]);\nassert_eq!(merge_intervals(vec![]), vec![]);"),
        "go": ("func MergeIntervals(xs [][2]int) [][2]int",
               "check(reflect.DeepEqual(MergeIntervals([][2]int{{1,3},{2,6},{8,10},{15,18}}), [][2]int{{1,6},{8,10},{15,18}}))\ncheck(reflect.DeepEqual(MergeIntervals([][2]int{{1,4},{4,5}}), [][2]int{{1,5}}))\ncheck(len(MergeIntervals(nil)) == 0)"),
    }),
    ("converts a Roman numeral (up to 3999) to an integer", {
        "python": ("def roman_to_int(s: str) -> int",
                   "assert roman_to_int('MCMXCIV')==1994\nassert roman_to_int('LVIII')==58\nassert roman_to_int('IX')==9"),
        "rust": ("fn roman_to_int(s: &str) -> i32",
                 "assert_eq!(roman_to_int(\"MCMXCIV\"), 1994);\nassert_eq!(roman_to_int(\"LVIII\"), 58);\nassert_eq!(roman_to_int(\"IX\"), 9);"),
        "go": ("func RomanToInt(s string) int",
               "check(RomanToInt(\"MCMXCIV\") == 1994)\ncheck(RomanToInt(\"LVIII\") == 58)\ncheck(RomanToInt(\"IX\") == 9)"),
    }),
    ("returns the k most frequent words, most frequent first, ties broken alphabetically", {
        "python": ("def top_k_frequent(words: list[str], k: int) -> list[str]",
                   "assert top_k_frequent(['i','love','leetcode','i','love','coding'],2)==['i','love']\nassert top_k_frequent(['the','day','is','sunny','the','the','the','sunny','is','is'],4)==['the','is','sunny','day']"),
        "rust": ("fn top_k_frequent(words: &[&str], k: usize) -> Vec<String>",
                 "assert_eq!(top_k_frequent(&[\"i\",\"love\",\"leetcode\",\"i\",\"love\",\"coding\"],2), vec![\"i\",\"love\"]);\nassert_eq!(top_k_frequent(&[\"the\",\"day\",\"is\",\"sunny\",\"the\",\"the\",\"the\",\"sunny\",\"is\",\"is\"],4), vec![\"the\",\"is\",\"sunny\",\"day\"]);"),
        "go": ("func TopKFrequent(words []string, k int) []string",
               "check(reflect.DeepEqual(TopKFrequent([]string{\"i\",\"love\",\"leetcode\",\"i\",\"love\",\"coding\"},2), []string{\"i\",\"love\"}))\ncheck(reflect.DeepEqual(TopKFrequent([]string{\"the\",\"day\",\"is\",\"sunny\",\"the\",\"the\",\"the\",\"sunny\",\"is\",\"is\"},4), []string{\"the\",\"is\",\"sunny\",\"day\"}))"),
    }),
    ("turns a duration like '1h30m', '45s' or '2h5s' (hours, minutes, seconds, each optional) into seconds", {
        "python": ("def parse_duration(s: str) -> int",
                   "assert parse_duration('1h30m')==5400\nassert parse_duration('45s')==45\nassert parse_duration('2h5s')==7205\nassert parse_duration('10m')==600"),
        "rust": ("fn parse_duration(s: &str) -> u64",
                 "assert_eq!(parse_duration(\"1h30m\"), 5400);\nassert_eq!(parse_duration(\"45s\"), 45);\nassert_eq!(parse_duration(\"2h5s\"), 7205);\nassert_eq!(parse_duration(\"10m\"), 600);"),
        "go": ("func ParseDuration(s string) int",
               "check(ParseDuration(\"1h30m\") == 5400)\ncheck(ParseDuration(\"45s\") == 45)\ncheck(ParseDuration(\"2h5s\") == 7205)\ncheck(ParseDuration(\"10m\") == 600)"),
    }),
    ("checks that (), [] and {} are balanced and properly nested", {
        "python": ("def valid_brackets(s: str) -> bool",
                   "assert valid_brackets('()[]{}')\nassert not valid_brackets('(]')\nassert valid_brackets('{[()()]}')\nassert not valid_brackets('((')"),
        "rust": ("fn valid_brackets(s: &str) -> bool",
                 "assert!(valid_brackets(\"()[]{}\"));\nassert!(!valid_brackets(\"(]\"));\nassert!(valid_brackets(\"{[()()]}\"));\nassert!(!valid_brackets(\"((\"));"),
        "go": ("func ValidBrackets(s string) bool",
               "check(ValidBrackets(\"()[]{}\"))\ncheck(!ValidBrackets(\"(]\"))\ncheck(ValidBrackets(\"{[()()]}\"))\ncheck(!ValidBrackets(\"((\"))"),
    }),
    ("evaluates reverse Polish notation with + - * / on integers (division truncates toward zero)", {
        "python": ("def eval_rpn(tokens: list[str]) -> int",
                   "assert eval_rpn(['2','1','+','3','*'])==9\nassert eval_rpn(['4','13','5','/','+'])==6\nassert eval_rpn(['10','6','9','3','+','-11','*','/','*','17','+','5','+'])==22"),
        "rust": ("fn eval_rpn(tokens: &[&str]) -> i64",
                 "assert_eq!(eval_rpn(&[\"2\",\"1\",\"+\",\"3\",\"*\"]), 9);\nassert_eq!(eval_rpn(&[\"4\",\"13\",\"5\",\"/\",\"+\"]), 6);\nassert_eq!(eval_rpn(&[\"10\",\"6\",\"9\",\"3\",\"+\",\"-11\",\"*\",\"/\",\"*\",\"17\",\"+\",\"5\",\"+\"]), 22);"),
        "go": ("func EvalRPN(tokens []string) int",
               "check(EvalRPN([]string{\"2\",\"1\",\"+\",\"3\",\"*\"}) == 9)\ncheck(EvalRPN([]string{\"4\",\"13\",\"5\",\"/\",\"+\"}) == 6)\ncheck(EvalRPN([]string{\"10\",\"6\",\"9\",\"3\",\"+\",\"-11\",\"*\",\"/\",\"*\",\"17\",\"+\",\"5\",\"+\"}) == 22)"),
    }),
    ("sorts version strings like '1.10.0' and '1.2.3' numerically by major, minor, patch", {
        "python": ("def semver_sort(vs: list[str]) -> list[str]",
                   "assert semver_sort(['1.10.0','1.2.3','1.2.10','0.9.9'])==['0.9.9','1.2.3','1.2.10','1.10.0']"),
        "rust": ("fn semver_sort(vs: &[&str]) -> Vec<String>",
                 "assert_eq!(semver_sort(&[\"1.10.0\",\"1.2.3\",\"1.2.10\",\"0.9.9\"]), vec![\"0.9.9\",\"1.2.3\",\"1.2.10\",\"1.10.0\"]);"),
        "go": ("func SemverSort(vs []string) []string",
               "check(reflect.DeepEqual(SemverSort([]string{\"1.10.0\",\"1.2.3\",\"1.2.10\",\"0.9.9\"}), []string{\"0.9.9\",\"1.2.3\",\"1.2.10\",\"1.10.0\"}))"),
    }),
]

LANG = {"python": "Python", "rust": "Rust", "go": "Go"}


def prompt(lang, desc, sig):
    extra = {
        "python": "Use only the standard library.",
        "rust": "Use only the standard library. Don't write a main function or tests.",
        "go": "Write the whole file in package main with its imports, using only the standard library. Don't write a main function or tests.",
    }[lang]
    return f"Write in {LANG[lang]} `{sig}` that {desc}. {extra} Reply with only the code in one fenced block."


def code_of(text):
    m = re.findall(r"```[a-zA-Z]*\n(.*?)```", text, re.S)
    if m:
        return m[0]
    # An unclosed fence: keep what follows it.
    return re.sub(r"^\s*```[a-zA-Z]*\n", "", text)


def run(name, lang, code, tests, tag):
    """Whether `code` passes `tests`; if not, it is kept with the output."""
    ok, log = run_(lang, code, tests)
    if not ok:
        with open(out_path("fail", name, f"{tag}.txt"), "w") as f:
            f.write(code + "\n----\n" + log)
    return ok


def run_(lang, code, tests):
    d = tempfile.mkdtemp()
    try:
        if lang == "python":
            open(f"{d}/t.py", "w").write(code + "\n\n" + tests + "\nprint('PASS')\n")
            p = subprocess.run(["python3", "t.py"], cwd=d, capture_output=True, text=True, timeout=30); out = p.stdout + p.stderr
        elif lang == "rust":
            body = "\n".join("    " + l for l in tests.splitlines())
            open(f"{d}/t.rs", "w").write(code + "\n\n#[allow(unused)]\nfn main() {\n" + body + "\n    println!(\"PASS\");\n}\n")
            c = subprocess.run(["rustc", "--edition", "2021", "-O", "-o", "t", "t.rs"], cwd=d, capture_output=True, text=True, timeout=120)
            if c.returncode != 0:
                return False, c.stderr
            p = subprocess.run(["./t"], cwd=d, capture_output=True, text=True, timeout=30); out = p.stdout + p.stderr
        else:
            if not code.lstrip().startswith("package"):
                code = "package main\n\n" + code
            open(f"{d}/a.go", "w").write(code)
            body = "\n".join("\t" + l for l in tests.splitlines())
            open(f"{d}/check.go", "w").write(
                "package main\n\nimport (\n\t\"fmt\"\n\t\"reflect\"\n)\n\nvar _ = reflect.DeepEqual\n\n"
                "func check(ok bool) {\n\tif !ok {\n\t\tpanic(\"fail\")\n\t}\n}\n\n"
                "func main() {\n" + body + "\n\tfmt.Println(\"PASS\")\n}\n")
            open(f"{d}/go.mod", "w").write("module t\n\ngo 1.21\n")
            p = subprocess.run(["go", "run", "."], cwd=d, capture_output=True, text=True, timeout=120,
                               env={**os.environ, "GOFLAGS": "-mod=mod",
                                    "GOCACHE": os.environ.get("GOCACHE", os.path.join(tempfile.gettempdir(), "gocache")),
                                    "GOPATH": os.environ.get("GOPATH", os.path.join(tempfile.gettempdir(), "gopath"))}); out = p.stdout + p.stderr
        return "PASS" in out, out
    except subprocess.TimeoutExpired:
        return False, "timeout"


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    name, args = sys.argv[1], sys.argv[2:]
    for tool in ("python3", "rustc", "go"):
        if not shutil.which(tool):
            sys.exit(f"codeeval: {tool} is not installed (the answers are compiled and run here)")
    passed = {l: 0 for l in LANG}
    wall, speeds, tokens = 0.0, [], 0
    with server(f"c-{name}", args, PORT) as load:
        for n, (desc, langs) in enumerate(TASKS):
            for lang, (sig, tests) in langs.items():
                r, took = chat(PORT, {"messages": [
                    {"role": "system", "content": "You are an expert programmer."},
                    {"role": "user", "content": prompt(lang, desc, sig)}],
                    "temperature": 0.2, "max_tokens": 8000 if THINK else 1500, **no_thinking(THINK)})
                wall += took
                t = r.get("timings", {})
                speeds.append(t.get("predicted_per_second", 0))
                tokens += t.get("predicted_n", 0)
                text = r["choices"][0]["message"].get("content") or ""
                passed[lang] += run(name, lang, code_of(text), tests, f"{lang}-{n}")
    speeds.sort()
    n = len(TASKS)
    print(f"{name + (' think' if THINK else ''):26} total {sum(passed.values()):2}/{3*n}  rust {passed['rust']}/{n}  "
          f"go {passed['go']}/{n}  python {passed['python']}/{n}  answer time {wall:6.1f} s  "
          f"median {speeds[len(speeds)//2]:6.1f} tok/s  load {load:5.1f} s  tokens {tokens}", flush=True)


if __name__ == "__main__":
    main()
