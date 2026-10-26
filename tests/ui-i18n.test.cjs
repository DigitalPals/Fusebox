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
  assert.equal(tr('5-hour window resets Wednesday, 14 October 2026 at 07:39:18 CEST'),
    '5 小时窗口将于 2026年10月14日 星期三 07:39:18 CEST 重置');
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

test('answers are remembered, up to a bound, so unmatched text is not tried twice', () => {
  const context = load();
  let tries = 0;
  // A pattern that matches nothing and counts how often it is asked.
  context.FUSEBOX_I18N.zh.patterns.unshift([{ test: () => { tries++; return false; } }, '']);
  const { tr } = context.FUSEBOX_I18N;
  assert.equal(tr('Something nobody wrote down'), 'Something nobody wrote down');
  assert.equal(tries, 1);
  assert.equal(tr('  Something nobody wrote down '), '  Something nobody wrote down ', 'the same text with other spacing');
  assert.equal(tries, 1, 'an unmatched line is not tried again');
  assert.equal(tr('Overview'), '概览');
  assert.equal(tr('Overview'), '概览');
  for (let i = 0; i < 5000; i++) tr(`Unknown line ${i}`);
  const before = tries;
  tr('Something nobody wrote down');
  assert.equal(tries, before + 1, 'old answers make room for new ones');
});

test('the dictionary is fetched from a URL the server can version', () => {
  const source = fs.readFileSync(path.join(__dirname, '../ui/i18n.js'), 'utf8');
  assert.match(source, /'\/ui\/i18n-zh\.js'/);
});

test('page refresh gestures and the discard confirmation read in Chinese', () => {
  const { tr } = load().FUSEBOX_I18N;
  assert.equal(tr('Refresh page'), '刷新页面');
  assert.equal(tr('Pull to refresh'), '下拉刷新');
  assert.equal(tr('Release to refresh'), '松开刷新');
  assert.equal(tr('Refreshing…'), '正在刷新…');
  assert.equal(tr('Reload and discard changes?'), '重新加载并放弃更改？');
  assert.equal(tr('Refreshing the page will discard your unsaved Config changes.'), '刷新页面将放弃尚未保存的配置更改。');
});

test('quota attribution and the usage database read in Chinese', () => {
  const { tr } = load().FUSEBOX_I18N;
  assert.equal(tr('Used by'), '使用方');
  assert.equal(tr('Each rise in the window’s reported use is split over the requests that finished since the previous rise, by their API list price. About 54% of it was split by tokens, as a model has no list price.'),
    '窗口报告用量的每次上升，都按 API 标价分摊给上一次上升以来完成的请求。其中约 54% 因模型没有标价而按 token 分摊。');
  assert.equal(tr('Estimated from quota readings · partly by tokens'), '根据额度读数估算 · 部分按 token 分摊');
  assert.equal(tr('3 more clients'), '另外 3 个客户端');
  assert.equal(tr('86% used · renewed Sat Oct 17'), '已用 86% · 10月17日 周六 已重置');
  assert.equal(tr('12% of 5h · 4% of week · <1% of week opus'), '5 小时窗口的 12% · 每周窗口的 4% · 每周 opus窗口的 <1%');
  assert.equal(tr('Opus weekly window'), 'Opus 每周窗口');
  assert.equal(tr('Weekly limit runs out early at this pace'), '每周额度按当前速度会提前用完');
  assert.equal(tr('It would run out before it renews. The laptop used 61% of it.'), '按当前速度，它会在重置前用完。The laptop 用掉了其中的 61%。');
  assert.equal(tr('Disk almost full'), '磁盘空间即将用完');
  assert.equal(tr('It is 900 MB and grows about 12 MB a day. Lower usage.retention-days, or raise usage.warn-database-mb.'),
    '数据库已有 900 MB，每天约增长 12 MB。请调低 usage.retention-days，或调高 usage.warn-database-mb。');
  assert.equal(tr('4.6M in · 870.0k out · 2.13B cached'), '输入 4.6M · 输出 870.0k · 缓存 2.13B');
  assert.equal(tr('Top clients'), '主要客户端');
  assert.equal(tr('$12.00 · 4 calls · 40% of this client'), '$12.00 · 4 次调用 · 占此客户端的 40%');
  assert.equal(tr('1,000 input, 200 output, 9,000 read from the cache, 300 written to the cache'), '输入 1,000，输出 200，缓存读取 9,000，缓存写入 300');
  assert.equal(tr('75% · 10.5k tokens'), '75% · 10.5k token');
});

test('combined weekly limits read in Chinese', () => {
  const { tr } = load().FUSEBOX_I18N;
  assert.equal(tr('Combined weekly limits'), '每周额度合计');
  assert.equal(tr('3 accounts'), '3 个账号');
  assert.equal(tr('2 of 3 accounts reporting'), '2/3 个账号已报告');
  assert.equal(tr('pace 32%'), '均匀 32%');
  assert.equal(tr('pace <1%'), '均匀 <1%');
  assert.equal(tr('10 points under pace'), '低于均匀用量 10 个百分点');
  assert.equal(tr('1 point over pace'), '高于均匀用量 1 个百分点');
  assert.equal(tr('On pace'), '符合均匀用量');
  assert.equal(tr('Claude combined weekly quota used'), 'Claude 每周合计额度已用');
  assert.equal(tr('22% used · Healthy · even pace 32% used by now').split(' · ')[2], '均匀用量此刻应为 32%');
  assert.equal(tr('3 accounts · Max 20x'), '3 个账号 · Max 20x', 'plan names stay as the provider writes them');
  assert.equal(tr('Each provider’s subscriptions together, weighted by plan price'), '每个提供商的订阅合并计算，按套餐价格加权');
  assert.equal(tr('Claude weighted by plan price, Codex equally'), 'Claude 按套餐价格加权，Codex 权重相同');
  assert.match(tr('Each account counts by its plan’s monthly price: Claude Pro 1, Max 5x 5, Max 20x 10'), /^每个账号按套餐月费计权/);
});

test('model setup and scoped quota explanations translate without changing model identifiers', () => {
  const { tr } = load().FUSEBOX_I18N;
  assert.equal(tr('Context limit unknown'),'上下文上限未知');
  assert.equal(tr('128,000 context tokens · Image input supported'),'128,000 个上下文 token · 支持图片输入');
  assert.equal(tr('Individual spending limit'),'个人支出上限');
  assert.equal(tr('Applies to all models'),'适用于所有模型');
  assert.equal(tr('Model: team/coding'),'模型：team/coding');
  assert.equal(tr('Model metadata overrides'),'模型元数据覆盖');
  assert.equal(tr('Download file'),'下载文件');
});

test('weekly forecasts translate reserve, learning and gap messages', () => {
  const { tr } = load().FUSEBOX_I18N;
  assert.equal(tr('Weekly quota outlook'), '每周额度预测');
  assert.equal(tr('Expected to last'), '预计额度充足');
  assert.equal(tr('Learning usage'), '正在学习用量');
  assert.equal(tr('Current quota while learning'), '学习期间显示当前额度');
  assert.equal(tr('remaining now'), '当前剩余');
  assert.equal(tr('Claude current combined weekly quota used'), 'Claude 当前每周合计额度已用');
  assert.equal(tr('Quota readings are stale'), '额度记录已过期');
  assert.equal(tr('Claude lowest projected weekly reserve'), 'Claude 预计最低每周剩余额度');
  assert.equal(tr('29% lowest projected reserve over the next 7 days'), '未来 7 天预计最低剩余额度 29%');
});
