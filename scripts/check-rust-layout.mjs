// Checks the Rust layout rules Clippy cannot: a per-file line limit, no #[path], no include!, no re-exports.
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';

const LIMIT = 1000;
const root = execFileSync('git', ['rev-parse', '--show-toplevel'], { encoding: 'utf8' }).trim();
const rel = (file) => path.relative(root, file).split(path.sep).join('/');
const metadata = JSON.parse(
  execFileSync('cargo', ['metadata', '--no-deps', '--format-version', '1'], {
    cwd: root,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  }),
);
const failures = [];

const lineCount = (text) => text.split('\n').length - (text.endsWith('\n') ? 1 : 0);
const MOD = /^\s*(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+(r#)?(\w+)\s*;/;
const PATH_ATTR = /#\[\s*path\s*=\s*"([^"]+)"\s*\]/;

// Every file a crate root pulls in through `mod x;`, the way rustc resolves them.
// shortcut: `mod x;` nested inside an inline `mod y { }` is not resolved; it
// fails as missing, so a crate that needs it must update this function.
function crateFiles(rootFile) {
  const seen = new Set();
  const visit = (file, isRoot) => {
    if (seen.has(file)) return;
    seen.add(file);
    const lines = readFileSync(file, 'utf8').split('\n');
    const base = path.basename(file, '.rs');
    const dir =
      isRoot || base === 'mod' ? path.dirname(file) : path.join(path.dirname(file), base);
    lines.forEach((line, index) => {
      const match = MOD.exec(line);
      if (!match) return;
      const attr = PATH_ATTR.exec(lines[index - 1] ?? '');
      const candidates = attr
        ? [path.join(path.dirname(file), attr[1])]
        : [path.join(dir, `${match[2]}.rs`), path.join(dir, match[2], 'mod.rs')];
      const found = candidates.find((candidate) => existsSync(candidate));
      if (found) visit(found, false);
      else failures.push(`${rel(file)}:${index + 1}: cannot find the file for \`mod ${match[2]};\``);
    });
  };
  visit(rootFile, true);
  return [...seen];
}

// The limit is per file, not per crate: a crate may hold many modules, and size
// alone is never a reason to split one (see CODE_STANDARDS.md).
let crates = 0;
const checked = new Set();
for (const pkg of metadata.packages) {
  for (const target of pkg.targets) {
    crates += 1;
    for (const file of crateFiles(target.src_path)) {
      if (checked.has(file)) continue;
      checked.add(file);
      const lines = lineCount(readFileSync(file, 'utf8'));
      if (lines > LIMIT) {
        failures.push(`${rel(file)}: ${pkg.name} has ${lines} lines in one file (limit ${LIMIT})`);
      }
    }
  }
}

const RULES = [
  { pattern: /#!?\[\s*path\b/, message: '#[path] attribute' },
  { pattern: /(?<![\w])include!\s*\(/, message: 'include! (use include_str! or include_bytes!)' },
  { pattern: /\bpub(\s*\([^)]*\))?\s+(use|extern\s+crate)\b/, message: 're-export' },
];
const sources = execFileSync('git', ['ls-files', '-z', '--', '*.rs'], { cwd: root, encoding: 'utf8' })
  .split('\0')
  .filter(Boolean);
// Blanks string literals and comments (keeping newlines, so line numbers hold)
// so that text about a rule never trips it.
// shortcut: raw strings with `"` inside (r#"…"#) and nested block comments are
// not handled; upgrade if the check ever misreads one.
const STRINGS_AND_COMMENTS = /"(?:\\.|[^"\\])*"|\/\/[^\n]*|\/\*[\s\S]*?\*\//g;
const codeOnly = (text) =>
  text.replace(STRINGS_AND_COMMENTS, (match) => match.replace(/[^\n]/g, ' '));

for (const file of sources) {
  codeOnly(readFileSync(path.join(root, file), 'utf8'))
    .split('\n')
    .forEach((line, index) => {
      for (const rule of RULES) {
        if (rule.pattern.test(line)) failures.push(`${file}:${index + 1}: ${rule.message}`);
      }
    });
}

if (failures.length > 0) {
  console.error(failures.join('\n'));
  console.error(`Rust layout check failed: ${failures.length} problem(s).`);
  process.exit(1);
}
console.log(`Rust layout check passed: ${crates} crates, ${sources.length} source files.`);
