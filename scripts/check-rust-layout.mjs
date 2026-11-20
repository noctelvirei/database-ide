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
function lexRust(text) {
  const comments = [];
  const code = text.split('');
  let i = 0;
  let line = 1;
  const blank = (from, to) => {
    for (let k = from; k < to; k++) {
      if (code[k] === '\n') line += 1;
      else code[k] = ' ';
    }
  };
  while (i < text.length) {
    const rest = text.slice(i, i + 40);
    let end;
    if (rest.startsWith('//')) {
      end = text.indexOf('\n', i);
      if (end === -1) end = text.length;
      comments.push({ line, text: text.slice(i, end).replace(/\r$/, '') });
    } else if (rest.startsWith('/*')) {
      let depth = 0;
      end = i;
      do {
        if (text.startsWith('/*', end)) {
          depth += 1;
          end += 2;
        } else if (text.startsWith('*/', end)) {
          depth -= 1;
          end += 2;
        } else end += 1;
      } while (depth > 0 && end < text.length);
      comments.push({ line, text: text.slice(i, end) });
    } else if (/^(?:b|c)?r#*"/.test(rest) && !/\w/.test(text[i - 1] ?? '')) {
      const hashes = /^(?:b|c)?r(#*)"/.exec(rest);
      const close = text.indexOf(`"${hashes[1]}`, i + hashes[0].length);
      end = close === -1 ? text.length : close + 1 + hashes[1].length;
    } else if (text[i] === '"') {
      end = i + 1;
      while (end < text.length && text[end] !== '"') end += text[end] === '\\' ? 2 : 1;
      end += 1;
    } else if (text[i] === "'") {
      const char = /^'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]+\}|.)|[^\\'\n])'/u.exec(rest);
      i += char ? char[0].length : 1;
      continue;
    } else {
      if (text[i] === '\n') line += 1;
      i += 1;
      continue;
    }
    blank(i, end);
    i = end;
  }
  return { code: code.join(''), comments };
}

const isExemptComment = (comment) =>
  /^\/\/[/!](?!\/)/.test(comment.text) ||
  /^\/\*[*!](?![*/])/.test(comment.text) ||
  /^\/\/\s*SAFETY:/.test(comment.text);

const allowance = JSON.parse(
  readFileSync(path.join(root, 'scripts/comment-allowance.json'), 'utf8'),
);
for (const file of Object.keys(allowance)) {
  if (!sources.includes(file)) failures.push(`scripts/comment-allowance.json: ${file} is not a tracked Rust file; remove it`);
}

for (const file of sources) {
  const { code, comments } = lexRust(readFileSync(path.join(root, file), 'utf8'));
  code.split('\n').forEach((line, index) => {
    for (const rule of RULES) {
      if (rule.pattern.test(line)) failures.push(`${file}:${index + 1}: ${rule.message}`);
    }
  });
  const plain = comments.filter((comment) => !isExemptComment(comment));
  const allowed = allowance[file] ?? 0;
  if (plain.length > allowed) {
    const lines = plain.map((comment) => comment.line).join(', ');
    failures.push(
      `${file}: ${plain.length} plain comments (lines ${lines}), ${allowed} allowed; ` +
        'put decisions in docs/adr/ and workarounds in the tracker (CODE_STANDARDS.md, Comments)',
    );
  } else if (plain.length < allowed) {
    failures.push(
      `${file}: ${plain.length} plain comments but ${allowed} allowed; ` +
        `set its entry in scripts/comment-allowance.json to ${plain.length || 'nothing (remove it)'}`,
    );
  }
}

if (failures.length > 0) {
  console.error(failures.join('\n'));
  console.error(`Rust layout check failed: ${failures.length} problem(s).`);
  process.exit(1);
}
console.log(`Rust layout check passed: ${crates} crates, ${sources.length} source files.`);
