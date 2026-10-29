const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');

test('model check messages distinguish pending checks, completed results and recent checks', async () => {
  const checkCode = source.slice(source.indexOf('async function checkModels('), source.indexOf('async function removeModel('));
  const pending = 'Some accounts are still being checked; results will appear automatically.';
  for (const [reply, expected] of [
    [{ asked: 0, in_progress: 2, new: [] }, pending],
    [{ asked: 2, in_progress: 1, new: [] }, `Asked 2 accounts; nothing new. ${pending}`],
    [{ asked: 0, in_progress: 1, new: ['new-model'] }, `New: new-model. ${pending}`],
    [{ asked: 0, in_progress: 0, new: ['new-model'] }, 'New: new-model.'],
    [{ asked: 0, in_progress: 0, new: [] }, 'Every account was checked in the last 10 minutes.'],
    [{ asked: 1, new: [] }, 'Asked 1 account; nothing new.'],
  ]) {
    let refreshes = 0, patches = 0;
    const context = { S: { addModel: {} }, api: async () => reply, $: () => null,
      plural: (n, label) => `${n} ${label}${n === 1 ? '' : 's'}`,
      refreshAccounts: () => refreshes++, patchModelsPanel: () => patches++ };
    vm.createContext(context);
    vm.runInContext(checkCode, context);
    await vm.runInContext('checkModels({})', context);
    assert.equal(context.S.addModel.msg, expected);
    assert.equal(context.S.addModel.err, false);
    assert.equal(refreshes, 1);
    assert.equal(patches, 1);
  }
});
const code = source.slice(source.indexOf('const routeSteps ='), source.indexOf('function failuresByModel()'));
function dashboard(models, routes, accounts) {
  let routeReads = 0;
  const context = { S: { models, routes: { models: new Proxy(routes, { get(target, key) { routeReads++; return target[key]; } }) }, accounts },
    accountById: (id) => accounts.find((a) => a.id === id) };
  vm.createContext(context);
  vm.runInContext(code, context);
  return { run: (ids) => JSON.parse(vm.runInContext(`JSON.stringify(famRoute({ ids: ${JSON.stringify(ids)} }))`, context)), reads: () => routeReads };
}

// Independent account-by-account reference for ordering, distinct-model coverage,
// and the metadata inherited from each account's first route.
function expected(ids, models, routes, accounts) {
  const steps = (id) => routes[id] || [];
  const base = (id) => { const prefix = models.find((m) => m.id === id)?.prefix; return prefix ? id.slice(prefix.length + 1) : id; };
  const rep = ids.reduce((best, id) => steps(id).length > steps(best).length ? id : best, ids[0]);
  const order = [...steps(rep).map((step) => ({ ...step }))];
  for (const id of ids) for (const step of steps(id)) {
    if (!order.some((row) => row.account === step.account)) order.push({ ...step, next: false });
  }
  return { ...(rep === undefined ? {} : { rep }), models: new Set(ids.map(base)).size, rows: order.filter((row) => accounts.some((a) => a.id === row.account)).map((row) => ({ ...row,
    serves: new Set(ids.filter((id) => steps(id).some((step) => step.account === row.account)).map(base)).size,
  })) };
}

test('family routes preserve ordering, state, missing accounts and shared prefixed-model coverage', () => {
  const models = [{ id: 'a' }, { id: 'b' }, { id: 'team/a', prefix: 'team' }, { id: 'other/a', prefix: 'other' }, { id: 'empty' }];
  const routes = {
    a: [{ account: 'one', state: 'ready', next: true }, { account: 'missing', state: 'disabled', next: false }],
    b: [{ account: 'two', state: 'cooling', until: '2026-10-30', next: false }],
    'team/a': [{ account: 'one', state: 'disabled', next: true }, { account: 'two', state: 'ready', next: true }],
    'other/a': [{ account: 'three', state: 'ready', next: true }],
  };
  const accounts = ['one', 'two', 'three'].map((id) => ({ id }));
  const page = dashboard(models, routes, accounts);
  for (const ids of [models.map((m) => m.id), ['b', 'team/a', 'a'], ['empty'], []]) {
    assert.deepEqual(page.run(ids), expected(ids, models, routes, accounts));
  }
  assert.equal(routes['other/a'][0].next, true, 'rendering never changes server route snapshots');
  assert.equal(routes.a[0].serves, undefined);
});

test('large ragged families visit each model route a fixed number of times', () => {
  const models = Array.from({ length: 120 }, (_, i) => ({ id: i < 60 ? `m${i}` : `team/m${i - 60}`, ...(i < 60 ? {} : { prefix: 'team' }) }));
  const accounts = Array.from({ length: 40 }, (_, i) => ({ id: `a${i}` }));
  const routes = Object.fromEntries(models.map((m, i) => [m.id, accounts.filter((_, j) => (i * 7 + j * 11) % 5 < 3).map((a, j) => ({ account: a.id, state: j % 2 ? 'ready' : 'cooling', next: j === 0 }))]));
  const ids = models.map((m) => m.id), page = dashboard(models, routes, accounts);
  assert.deepEqual(page.run(ids), expected(ids, models, routes, accounts));
  assert.ok(page.reads() <= ids.length * 3 + 1, `route reads should scale with models, got ${page.reads()}`);
});
