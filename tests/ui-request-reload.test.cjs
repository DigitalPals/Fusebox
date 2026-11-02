const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
const start = source.indexOf('let overviewAt =');
const end = source.indexOf('// Route order follows account state', start);
assert.ok(start >= 0 && end > start);
const settle = () => new Promise(setImmediate);

function dashboard() {
  const calls = [];
  const S = { route: 'requests', requests: [{ id: 1 }], requestsFull: false, locked: null };
  const context = { S,
    api(endpoint) { return new Promise((resolve) => calls.push({ endpoint, resolve })); },
    refreshRequests() {}, patch() {}, renderFaults() {},
  };
  vm.createContext(context);
  vm.runInContext(source.slice(start, end), context);
  return {
    S, calls,
    reload() { vm.runInContext('reloadRequests()', context); },
    load() { vm.runInContext('loadRequests()', context); },
    ids() { return Array.from(S.requests, (r) => r.id); },
  };
}

test('reloading requests preserves live arrivals that came after the snapshot', async () => {
  const d = dashboard();
  d.reload();
  // A live event arrives while the HTTP snapshot is still on its way.
  d.S.requests.unshift({ id: 3 }, { id: 2 });
  d.calls[0].resolve([{ id: 2 }, { id: 1 }]);
  await settle();
  assert.deepEqual(d.ids(), [3, 2, 1], 'the newer arrival remains, and the overlap is not duplicated');
  assert.equal(d.S.requestsFull, true);
});

test('an older overlapping reload cannot replace the newer snapshot', async () => {
  const d = dashboard();
  d.reload(); d.reload();
  d.calls[1].resolve([{ id: 3 }, { id: 2 }]);
  await settle();
  d.calls[0].resolve([{ id: 1 }]);
  await settle();
  assert.deepEqual(d.ids(), [3, 2]);
});

test('an initial full-log read cannot overwrite a newer reload', async () => {
  const d = dashboard();
  d.load(); d.reload();
  d.calls[1].resolve([{ id: 3 }]);
  await settle();
  d.calls[0].resolve([{ id: 1 }, { id: 2 }]);
  await settle();
  assert.deepEqual(d.ids(), [3]);
});

test('a reload cannot overwrite a newer full-log read', async () => {
  const d = dashboard();
  d.reload(); d.load();
  d.calls[1].resolve([{ id: 3 }, { id: 2 }]);
  await settle();
  d.calls[0].resolve([{ id: 1 }]);
  await settle();
  assert.deepEqual(d.ids(), [3, 2]);
});

test('a replacement snapshot drops old rows and preserves arrivals even when request IDs restarted', async () => {
  const d = dashboard();
  d.S.requests = [{ id: 1, ts: 'old-server' }, { id: 2, ts: 'old-server' }];
  d.reload();
  d.S.requests.unshift({ id: 1, ts: 'new-server' });
  d.calls[0].resolve([]);
  await settle();
  assert.deepEqual(d.ids(), [1]);
  assert.equal(d.S.requests[0].ts, 'new-server');
});

test('snapshot and live arrivals together stay within the request log bound', async () => {
  const d = dashboard();
  d.reload();
  d.S.requests.unshift({ id: 301 }, { id: 300 });
  d.calls[0].resolve(Array.from({ length: 300 }, (_, i) => ({ id: 300 - i })));
  await settle();
  assert.equal(d.S.requests.length, 300);
  assert.deepEqual(d.ids().slice(0, 3), [301, 300, 299]);
});
