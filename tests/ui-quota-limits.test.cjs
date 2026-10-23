const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');
const source = fs.readFileSync(path.join(__dirname,'../ui/app.js'),'utf8');
function piece(start) {
  const at = source.indexOf(start);
  return source.slice(at, start.startsWith('function') ? source.indexOf('\n}\n',at)+2 : source.indexOf(';\n',at)+1);
}
function load() {
  const ctx = { S:{quotaDisplay:'remaining'}, when:()=> 'tomorrow', meterHTML:(q)=>`<meter>${q.remaining}</meter>` };
  vm.createContext(ctx);
  vm.runInContext(['const esc =','function quotaOf(','function quotaPercent(','function quotaLimitsHTML(','const windowTitle =','const windowLabel =','function windowSeconds('].map(piece).join('\n'),ctx);
  return (limits,compact=false) => vm.runInContext(`quotaLimitsHTML(${JSON.stringify({quota:{limits}})},${compact})`,ctx);
}

test('spending limits show provider amounts and scope without inventing dollars or a duration', () => {
  const html = load()([{name:'Individual spending limit',scope:{kind:'account'},used:0,limit:20,used_percent:0,unit:'credits'}]);
  assert.match(html,/0 \/ 20 credits/);
  assert.match(html,/100%/);
  assert.match(html,/Applies to all models/);
  assert.match(html,/No reset time reported/);
  assert.doesNotMatch(html,/\$|5-hour|Weekly/);
});

test('zero credits, unknown balances, feature scopes, and optional overages stay distinct', () => {
  const render = load();
  assert.match(render([{name:'Credits',scope:{kind:'credits'},remaining:0}]),/0 left/);
  const unknown = render([{name:'Credits',scope:{kind:'credits'},remaining:null,unlimited:false}]);
  assert.match(unknown,/Not reported/);
  assert.doesNotMatch(unknown,/Unlimited|0 left/);
  assert.match(render([{name:'Code review',scope:{kind:'feature',value:'review'},used_percent:100}]),/Separate feature allowance/);
  assert.match(render([{name:'Extra usage',scope:{kind:'overage'},enabled:false}]),/Disabled.*Optional paid usage/s);
  assert.match(render([{name:'<script>',scope:{kind:'model',value:'<img>'}}]),/&lt;script&gt;.*Model: &lt;img&gt;/s);
});

test('expired financial limits await refresh rather than appearing exhausted', () => {
  const html = load()([{name:'Spending',scope:{kind:'account'},used_percent:100,resets_at:'2000-01-01T00:00:00Z'}]);
  assert.match(html,/Renewal passed; awaiting update/);
  assert.doesNotMatch(html,/<meter>|0%/);
});

test('an undated spending cap reports an unknown reset instead of a made-up countdown', () => {
  const ctx = { liveLeft:()=> '5m' };
  vm.createContext(ctx);
  vm.runInContext(piece('const SIGNIN_ERR =')+'\n'+piece('function acctState('),ctx);
  const state = vm.runInContext(`acctState({cooldowns:{'*':'2099-01-01T00:00:00Z'},cooldown_kinds:{'*':'quota'},quota_reset_unknown:true})`,ctx);
  assert.equal(state.resetUnknown,true);
  assert.equal(state.word,'Quota used up');
  assert.equal(state.html,undefined);
});
