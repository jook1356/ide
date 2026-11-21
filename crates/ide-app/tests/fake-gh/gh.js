// A fake `gh` for the app script's footer pass (`script_footer.rs`). Never touches the network.
//
// Answers the PR backend's `gh api --hostname <host> --include -X <METHOD> <path> [-f k=v ...]
// [--input -]` calls (`ide-server/src/pr.rs`, `GhCli::args`) like GitHub's REST / GraphQL API,
// printing `gh --include` output: `HTTP/2.0 <status> X`, a JSON content type, a blank line, the
// JSON body. Pull requests live in `state.json`; head / base commits are read from the local bare
// origin `IDE_FAKE_GH_ORIGIN`, and a merge squashes the head into the base there (and deletes the
// head branch when asked), so git sees what GitHub would have done.
//
// Env: `IDE_FAKE_GH_ORIGIN` (bare origin, required for merges), `IDE_FAKE_GH_STATE` (directory of
// `state.json` and `calls.log`; default: this script's directory).
// Endpoints: GET/POST repos/<o>/<r>/pulls, GET/PATCH repos/<o>/<r>/pulls/<n>,
// GET .../pulls/<n>/reviews, PUT .../pulls/<n>/merge, PUT .../pulls/<n>/update-branch,
// DELETE .../git/refs/heads/<branch>, POST graphql; anything else -> 404.
'use strict';

const fs = require('fs');
const path = require('path');
const { execFileSync } = require('child_process');

const stateDir = process.env.IDE_FAKE_GH_STATE || __dirname;
fs.mkdirSync(stateDir, { recursive: true });
const stateFile = path.join(stateDir, 'state.json');
const logFile = path.join(stateDir, 'calls.log');
const origin = process.env.IDE_FAKE_GH_ORIGIN || '';

function log(line) {
  fs.appendFileSync(logFile, `${new Date().toISOString()} ${line}\n`);
}

function reply(status, body) {
  log(`  -> ${status}`);
  const json = body === undefined ? '' : JSON.stringify(body);
  // Synchronous: the process exits right after.
  fs.writeSync(1, `HTTP/2.0 ${status} X\r\nContent-Type: application/json\r\n\r\n${json}`);
  process.exit(status >= 400 ? 1 : 0);
}

function load() {
  try {
    return JSON.parse(fs.readFileSync(stateFile, 'utf8'));
  } catch {
    return { next: 1, pulls: [] };
  }
}

function save(state) {
  fs.writeFileSync(stateFile, JSON.stringify(state, null, 1));
}

function git(args) {
  return execFileSync('git', ['-C', origin, ...args], {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
    env: {
      ...process.env,
      GIT_AUTHOR_NAME: 'fake-gh',
      GIT_AUTHOR_EMAIL: 'fake-gh@example.invalid',
      GIT_COMMITTER_NAME: 'fake-gh',
      GIT_COMMITTER_EMAIL: 'fake-gh@example.invalid',
    },
  }).trim();
}

/** The origin's commit of `branch`, or '' (no origin / no such branch). */
function sha(branch) {
  if (!origin) return '';
  try {
    return git(['rev-parse', '--verify', '-q', `refs/heads/${branch}^{commit}`]);
  } catch {
    return '';
  }
}

// ---- arguments -------------------------------------------------------------------------------

const argv = process.argv.slice(2);
log(`gh ${argv.join(' ')}`);
if (argv[0] !== 'api') {
  process.stderr.write(`fake gh: only \`gh api\` is supported (${argv.join(' ')})\n`);
  process.exit(2);
}
let method = 'GET';
let apiPath = '';
let input = false;
const query = {};
for (let i = 1; i < argv.length; i++) {
  const a = argv[i];
  if (a === '--hostname') i++;
  else if (a === '--include') continue;
  else if (a === '-X' || a === '--method') method = argv[++i].toUpperCase();
  else if (a === '-f' || a === '-F' || a === '--raw-field' || a === '--field') {
    const kv = argv[++i];
    const eq = kv.indexOf('=');
    query[kv.slice(0, eq)] = kv.slice(eq + 1);
  } else if (a === '--input') {
    input = argv[++i] === '-';
  } else if (!a.startsWith('-')) apiPath = a.replace(/^\/+/, '');
}
let body = {};
if (input) {
  const text = fs.readFileSync(0, 'utf8').trim();
  log(`  body ${text.length > 300 ? text.slice(0, 300) + '…' : text}`);
  body = text ? JSON.parse(text) : {};
}

// ---- pull requests ---------------------------------------------------------------------------

function htmlUrl(n, title) {
  const page =
    `<meta charset="utf-8"><title>PR ${n}</title><body style="background:#222;color:#ddd;font:20px sans-serif">` +
    `<h1>Fake PR #${n}</h1><p>${title.replace(/[<&]/g, ' ')}</p></body>`;
  return 'data:text/html;charset=utf-8,' + encodeURIComponent(page);
}

function view(pr, full) {
  const head = pr.state === 'open' ? sha(pr.head) || pr.head_sha : pr.head_sha;
  return {
    number: pr.number,
    state: pr.state,
    merged: pr.merged,
    draft: pr.draft,
    title: pr.title,
    body: pr.body,
    html_url: pr.html_url,
    mergeable: pr.state === 'open' ? true : null,
    mergeable_state: pr.state === 'open' ? 'clean' : 'unknown',
    merge_commit_sha: pr.merge_sha || null,
    head: { ref: pr.head, sha: head, repo: { full_name: full } },
    base: { ref: pr.base, sha: sha(pr.base), repo: { full_name: full } },
  };
}

const state = load();
const m = apiPath.match(/^repos\/([^/]+)\/([^/]+)\/(.*)$/);
if (apiPath === 'graphql' && method === 'POST') {
  const n = Number(body.variables && body.variables.number);
  const pr = state.pulls.find((p) => p.number === n);
  if (!pr) reply(200, { data: { repository: { pullRequest: null } }, errors: [{ message: `no PR #${n}` }] });
  reply(200, {
    data: {
      repository: {
        pullRequest: {
          headRefOid: sha(pr.head) || pr.head_sha,
          reviewDecision: null,
          mergeStateStatus: pr.state === 'open' ? 'CLEAN' : 'UNKNOWN',
          commits: { nodes: [{ commit: { statusCheckRollup: null } }] },
        },
      },
    },
  });
}
if (!m) reply(404, { message: 'Not Found' });
const full = `${m[1]}/${m[2]}`;
const rest = m[3];
const owner = m[1].toLowerCase();

if (rest === 'pulls' && method === 'GET') {
  let list = state.pulls.slice().sort((a, b) => b.number - a.number);
  if (query.state && query.state !== 'all') list = list.filter((p) => p.state === query.state);
  if (query.head) {
    const [o, ...b] = query.head.split(':');
    list = list.filter((p) => o.toLowerCase() === owner && p.head === b.join(':'));
  }
  reply(200, list.map((p) => view(p, full)));
}

if (rest === 'pulls' && method === 'POST') {
  for (const k of ['title', 'head', 'base']) {
    if (!body[k]) reply(422, { message: 'Validation Failed', errors: [{ message: `${k} is missing` }] });
  }
  const headSha = sha(body.head);
  if (!headSha) reply(422, { message: 'Validation Failed', errors: [{ message: `head ${body.head} is not on origin` }] });
  if (state.pulls.some((p) => p.state === 'open' && p.head === body.head)) {
    reply(422, {
      message: 'Validation Failed',
      errors: [{ message: `A pull request already exists for ${m[1]}:${body.head}.` }],
    });
  }
  const n = state.next++;
  const pr = {
    number: n,
    state: 'open',
    merged: false,
    draft: !!body.draft,
    title: body.title,
    body: body.body || '',
    head: body.head,
    base: body.base,
    head_sha: headSha,
    html_url: htmlUrl(n, body.title),
  };
  state.pulls.push(pr);
  save(state);
  reply(201, view(pr, full));
}

const one = rest.match(/^pulls\/(\d+)(\/(merge|reviews|update-branch))?$/);
if (one) {
  const pr = state.pulls.find((p) => p.number === Number(one[1]));
  if (!pr) reply(404, { message: 'Not Found' });
  const sub = one[3] || '';
  if (sub === '' && method === 'GET') reply(200, view(pr, full));
  if (sub === 'reviews' && method === 'GET') reply(200, []);
  if (sub === '' && method === 'PATCH') {
    if (body.state === 'closed' && pr.state === 'open') {
      pr.state = 'closed';
      pr.head_sha = sha(pr.head) || pr.head_sha;
    }
    if (body.title) pr.title = body.title;
    save(state);
    reply(200, view(pr, full));
  }
  if (sub === 'merge' && method === 'PUT') {
    if (pr.state !== 'open') reply(405, { message: 'Pull Request is not mergeable' });
    const head = sha(pr.head);
    if (body.sha && body.sha !== head) reply(409, { message: 'Head branch was modified. Review and try the merge again.' });
    const base = sha(pr.base);
    let tree;
    try {
      tree = git(['merge-tree', '--write-tree', base, head]).split('\n')[0];
    } catch (e) {
      reply(405, { message: `Merge conflict: ${String(e.stdout || e.message).split('\n').slice(1).join(' ')}` });
    }
    const merged = git(['commit-tree', tree, '-p', base, '-m', `${pr.title} (#${pr.number})`]);
    git(['update-ref', `refs/heads/${pr.base}`, merged, base]);
    Object.assign(pr, { state: 'closed', merged: true, head_sha: head, merge_sha: merged });
    save(state);
    reply(200, { sha: merged, merged: true, message: 'Pull Request successfully merged' });
  }
  if (sub === 'update-branch' && method === 'PUT') reply(202, { message: 'Updating pull request branch.' });
  reply(404, { message: 'Not Found' });
}

const ref = rest.match(/^git\/refs\/heads\/(.+)$/);
if (ref && method === 'DELETE') {
  const branch = ref[1].split('/').map(decodeURIComponent).join('/');
  if (!sha(branch)) reply(422, { message: 'Reference does not exist' });
  git(['update-ref', '-d', `refs/heads/${branch}`]);
  reply(204);
}

reply(404, { message: 'Not Found' });
