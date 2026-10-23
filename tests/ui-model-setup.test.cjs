const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '../ui/app.js'), 'utf8');
function piece(start) {
  const at = source.indexOf(start);
  assert.ok(at >= 0, start);
  return source.slice(at, start.startsWith('function') ? source.indexOf('\n}\n', at) + 2 : source.indexOf(';\n', at) + 1);
}
const known = { id: 'team/coding', capabilities: { context_window: 128000, max_output_tokens: 16000, input_modalities: ['text', 'image'], tool_calling: true }, codex_catalog: true };
const unknown = { id: 'budget/plain' };
const client = { id: 'laptop', key: 'fbx_laptop_key', models: ['team/*'] };
function load(models = [known, unknown], hidden = false) {
  const context = { S: { models, overview: { client_keys: ['never-a-shared-key'], named_clients: 1 }, private: hidden,
    config: { values: { 'named-clients': [JSON.parse(JSON.stringify(client))] } }, snippet: 'opencode', setupModels: Object.create(null), setupContexts: Object.create(null) },
    location: { origin: 'https://fusebox.test' }, ICON: { copy: '' }, setTimeout, Date };
  vm.createContext(context);
  vm.runInContext([piece('const esc ='), piece('const HIDDEN ='), piece('const secret ='),
    piece('const SNIPPET_TABS ='), piece('const SNIPPET_MORE ='),
    ...['setupModelsFor', 'setupModel', 'modelCapabilitiesText', 'snippet', 'setupHTML'].map((s) => piece(`function ${s}(`))].join('\n'), context);
  const run = (code) => vm.runInContext(code, context);
  return { run, state: context.S, snippet: (kind, key = null, selected = null) => run(`snippet(${JSON.stringify(kind)}, ${JSON.stringify(key)}, ${JSON.stringify(selected)})`) };
}

test('OpenCode exports carry the selected public alias, model limits, images and only that client key', () => {
  const page = load();
  const sn = page.snippet('opencode', client.key, known.id);
  const cfg = JSON.parse(sn.text);
  assert.equal(cfg.model, 'fusebox/team/coding');
  assert.deepEqual(cfg.provider.fusebox.models['team/coding'].limit, { context: 128000, output: 16000 });
  assert.deepEqual(cfg.provider.fusebox.models['team/coding'].modalities.input, ['text', 'image']);
  assert.equal(cfg.provider.fusebox.options.apiKey, client.key);
  assert.ok(!sn.text.includes('never-a-shared-key'));
  assert.equal(sn.filename, 'opencode.json');
});

test('named clients cannot select or export disallowed models, including wildcard punctuation', () => {
  const page = load();
  assert.deepEqual(JSON.parse(page.run(`JSON.stringify(setupModelsFor(${JSON.stringify(client.key)}).map(m=>m.id))`)), [known.id]);
  const sn = page.snippet('opencode', client.key, unknown.id);
  assert.equal(JSON.parse(sn.text).model, 'fusebox/team/coding');
  page.state.config.values['named-clients'][0].models = ['budget/*'];
  assert.equal(JSON.parse(page.snippet('opencode',client.key,known.id).text).model,'fusebox/budget/plain');
});

test('unknown limits stay unknown and Zed requires an explicit context before export', () => {
  const page = load([unknown]);
  const opencode = JSON.parse(page.snippet('opencode', null, unknown.id).text);
  assert.deepEqual(opencode.provider.fusebox.models[unknown.id], {});
  assert.equal(page.snippet('zed').filename, null);
  assert.match(page.run(`modelCapabilitiesText(S.models[0])`), /Context limit unknown.*Image support unknown/);
  page.state.setupContexts[unknown.id] = 64000;
  const zed = JSON.parse(page.snippet('zed').text);
  const model = zed.language_models.openai_compatible.Fusebox.available_models[0];
  assert.equal(model.max_tokens,64000);
  assert.equal(model.max_output_tokens,undefined);
  assert.equal(model.capabilities,undefined);
});

test('Zed and Continue receive reported limits and correctly named image/tool settings', () => {
  const page = load();
  const zed = JSON.parse(page.snippet('zed', client.key, known.id).text);
  assert.deepEqual(zed.language_models.openai_compatible.Fusebox.available_models[0], {
    name: known.id, max_tokens:128000,max_output_tokens:16000,
    capabilities:{tools:true,images:true,parallel_tool_calls:false,prompt_cache_key:false},
  });
  const cont = page.snippet('continue',client.key,known.id);
  assert.match(cont.text,/contextLength: 128000/);
  assert.match(cont.text,/maxTokens: 16000/);
  assert.match(cont.text,/capabilities: \["tool_use","image_input"\]/);
});

test('Codex uses full remote metadata only when it exists, and puts provider settings in their table', () => {
  const page = load();
  const sn = page.snippet('codex',client.key,known.id);
  assert.match(sn.text,/^model_context_window = 128000$/m);
  assert.match(sn.text,/\[model_providers.fusebox\][\s\S]*model_catalog_url = "https:\/\/fusebox.test\/v1\/codex\/models.json\?model=team%2Fcoding"/);
  assert.match(sn.text,/env_key = "FUSEBOX_KEY"/);
  assert.ok(!sn.text.includes(client.key));
  const other = page.snippet('codex',null,unknown.id);
  assert.doesNotMatch(other.text,/model_catalog_url|model_context_window/);
  assert.match(other.note,/built-in defaults/);
});

test('setup retains a chosen model and privacy masks the visible key without breaking copied configuration', () => {
  const page = load([known, { ...unknown, id:'team/plain' }],true);
  page.state.setupModels['laptop:opencode']='team/plain';
  const html = page.run(`setupHTML(${JSON.stringify(client.key)}, '-row')`);
  assert.match(html,/<option value="team\/plain" selected>/);
  assert.match(html,/Download file/);
  const sn = page.snippet('opencode',client.key,known.id);
  assert.ok(sn.text.includes(client.key));
  assert.ok(!sn.html.includes(client.key));
  page.state.config.values['named-clients'][0].models = ['nothing'];
  const empty = page.run(`setupHTML(${JSON.stringify(client.key)})`);
  assert.match(empty,/No models available for this client/);
  assert.doesNotMatch(empty,/data-act="download-setup"/);
});

test('quotes and shell metacharacters in model aliases remain data in JSON, TOML and shell examples', () => {
  const id = 'team/$(touch BAD)\'"<script>';
  const page = load([{ id }]);
  assert.equal(JSON.parse(page.snippet('opencode').text).provider.fusebox.models[id] != null,true);
  assert.match(page.snippet('codex').text,/model = "team\/\$\(touch BAD\)'\\"<script>"/);
  assert.match(page.snippet('claude').text,/ANTHROPIC_MODEL='team\/\$\(touch BAD\)'"'"'"<script>'/);
  assert.ok(!page.snippet('opencode').html.includes('<script>'));
});
