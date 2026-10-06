import test from 'node:test';
import assert from 'node:assert/strict';
import { alertLocation, alertRoute } from '../src/route.ts';

test('通知详情和摘要分别进入指定告警及告警中心', () => {
  const id = 'synthetic-run:100:v1:archive_output:engine:1';
  assert.deepEqual(alertRoute(`?alert=${encodeURIComponent(id)}`), { page: 'alerts', alert: id, invalid: false });
  assert.deepEqual(alertRoute('?view=alerts'), { page: 'alerts', invalid: false });
  assert.deepEqual(alertRoute(''), { page: 'overview', invalid: false });
});

test('非法通知标识不能变成导航或任意地址', () => {
  for (const id of ['', 'https://attacker.example', '../other', 'x\nvalue', 'x'.repeat(513)]) {
    assert.deepEqual(alertRoute(`?alert=${encodeURIComponent(id)}`), { page: 'alerts', alert: undefined, invalid: true });
  }
});

test('关闭详情保留列表入口和会话外的其他参数', () => {
  const location = { pathname: '/', search: '?alert=old&extra=synthetic', hash: '' };
  assert.equal(alertLocation(location, undefined, true), '/?extra=synthetic&view=alerts');
  assert.equal(alertLocation(location, 'new:1'), '/?extra=synthetic&alert=new%3A1');
  assert.equal(alertLocation(location), '/?extra=synthetic');
});
