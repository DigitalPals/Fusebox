const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

// The translation layer with the Chinese dictionary, run from their own sources: exact
// strings, scoped entries, patterns, " · " parts and whole sentences.
function load(languages = ['zh-CN'], saved = null) {
  const read = (name) => fs.readFileSync(path.join(__dirname, '../ui', name), 'utf8');
  const context = {
    navigator: { languages },
    localStorage: { getItem: () => saved, setItem() {} },
    location: { reload() {} },
    document: {
      readyState: 'loading',
      addEventListener() {},
      createElement: () => ({}),
      head: { append(script) { vm.runInContext(read(script.src.replace('/ui/', '')), context); script.onload(); } },
    },
  };
  context.window = context;
  vm.createContext(context);
  vm.runInContext(read('i18n.js'), context);
  return context;
}

test('the browser language picks Chinese, and a saved choice wins', () => {
  assert.equal(load().fuseboxLanguage.current, 'zh');
  assert.equal(load(['en-GB']).fuseboxLanguage.current, 'en');
  assert.equal(load(['zh-CN'], 'en').fuseboxLanguage.current, 'en');
  assert.equal(load(['en-GB']).FUSEBOX_I18N, undefined, 'English loads no dictionary');
});

test('exact strings, patterns and dates translate', () => {
  const { tr } = load().FUSEBOX_I18N;
  assert.equal(tr('Overview'), '概览');
  assert.equal(tr('  Accounts '), '  账号 ', 'surrounding space is kept');
  assert.equal(tr('Copy gpt-6-astra'), '复制 gpt-6-astra');
  assert.equal(tr('Remove API key 2'), '移除 API 密钥 2');
  assert.equal(tr('Resets Thu 12:39 · in'), '周四 12:39 重置 · 还有');
  assert.equal(tr('5-hour window resets Wednesday, 14 October 2026 at 07:39:18 CEST · using 12% an hour'),
    '5 小时窗口将于 2026年10月14日 星期三 07:39:18 CEST 重置 · 每小时用掉 12%');
  assert.equal(tr('Thu Oct 8 – Wed Oct 14'), '10月8日 周四 – 10月14日 周三');
  assert.equal(tr('Fri Oct 9: Unpriced'), '10月9日 周五：未定价');
  assert.equal(tr('Restart Fusebox to apply changes to port, log format.'), '重启 Fusebox 后，对端口、日志格式的修改才会生效。');
});

test('lines of parts and several sentences translate piece by piece', () => {
  const { tr } = load().FUSEBOX_I18N;
  assert.equal(tr('12% used · Healthy · even pace 11% used by now').split(' · ')[0], '已用 12%');
  assert.equal(tr('60 a minute · until Dec 31, 2026 · gpt-*'), '每分钟 60 次 · 有效期至 2026年12月31日 · gpt-*');
  assert.equal(tr('First request of this session. It went to the first available account, and the session stays there from now on. It took two tries.'),
    '这是此会话的第一个请求。交给了第一个可用账号，此后会话一直留在那里。尝试了两次。');
  assert.equal(tr('Something nobody wrote down'), 'Something nobody wrote down', 'unknown text stays English');
  assert.equal(tr('work@example.com · Unknown text'), 'work@example.com · Unknown text');
});
