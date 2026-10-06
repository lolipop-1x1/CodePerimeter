import test, { before, after } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createElement, type ComponentType } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer, type ViteDevServer } from 'vite';
import type { CollectorStreamSample, ServiceOperation, Status } from '../src/types.ts';

let server: ViteDevServer;
let cache: string;
let SourceDiagnostics: ComponentType<{ streams?: Record<string, CollectorStreamSample> }>;
let HealthStrip: ComponentType<{ status?: Status }>;
let stateLabel: (value: string) => string;
let errorMessage: (value: unknown) => string;
let SettingsPage: ComponentType<{ active: boolean; operation?: ServiceOperation; status?: Status; operate: () => void }>;

// 使用实际 React／Carbon 组件渲染匿名诊断样本，不启动 HTTP 或替代业务接口。
before(async () => {
  cache = mkdtempSync(join(tmpdir(), 'codeperimeter-component-test-'));
  server = await createServer({ configFile: false, appType: 'custom', cacheDir: cache, logLevel: 'silent', esbuild: { jsx: 'automatic' }, server: { middlewareMode: true, hmr: false, watch: null } });
  const settings = await server.ssrLoadModule('/src/settings.tsx');
  SourceDiagnostics = settings.CollectorSourceDiagnostics;
  SettingsPage = settings.SettingsPage;
  const overview = await server.ssrLoadModule('/src/overview.tsx');
  HealthStrip = overview.HealthStrip;
  stateLabel = overview.stateLabel;
  errorMessage = (await server.ssrLoadModule('/src/api.ts')).errorMessage;
});
after(async () => { await server?.close(); if (cache) rmSync(cache, { recursive: true, force: true }); });

test('来源诊断独立显示读取、写入与旧来源的最新样本延迟', () => {
  const markup = renderToStaticMarkup(createElement(SourceDiagnostics, { streams: {
    exec: { last_source_timestamp_ms: 1000, last_received_timestamp_ms: 21010 },
    read: { last_source_timestamp_ms: 1000, last_received_timestamp_ms: 1020 },
    write: { last_source_timestamp_ms: 1000, last_received_timestamp_ms: 1010 },
    activity: { last_source_timestamp_ms: 2000, last_received_timestamp_ms: 22000 },
    combined: { last_source_timestamp_ms: 0, last_received_timestamp_ms: 0 },
  } }));
  for (const label of ['命令', '打开／映射', '写入', '文件活动', '合并', '10 ms', '20 ms', '20,010 ms', '20,000 ms', '0 ms', '样本接收时间']) assert.ok(markup.includes(label));
  assert.ok(markup.includes(new Date(21010).toLocaleString('zh-CN', { hour12: false })));
});

test('时间缺失保持未知，负延迟明确标为时钟异常', () => {
  const markup = renderToStaticMarkup(createElement(SourceDiagnostics, { streams: {
    exec: { last_source_timestamp_ms: null, last_received_timestamp_ms: 3000 },
    activity: { last_source_timestamp_ms: 2050, last_received_timestamp_ms: 2000 },
    combined: { last_source_timestamp_ms: 2000, last_received_timestamp_ms: null },
  } }));
  assert.ok(markup.includes('时钟异常（-50 ms）'));
  assert.ok((markup.match(/<td[^>]*>未知<\/td>/g) ?? []).length >= 3);
  const empty = renderToStaticMarkup(createElement(SourceDiagnostics, {}));
  assert.ok(empty.includes('暂无来源样本，延迟与接收时间未知。'));
});

test('状态标签区分连接、缺口、等待、成功及未知，健康条不会把未知写成正常', () => {
  for (const [value, expected] of [['gap', '有缺口'], ['connecting', '连接中'], ['reconnecting', '重新连接中'], ['stalled', '事件流停滞'], ['coverage_gap', '覆盖有缺口'], ['pending', '等待中'], ['succeeded', '已成功'], ['cancelled', '已取消'], ['new_unknown_state', '未知状态']]) assert.equal(stateLabel(value), expected);
  const status = { service: { installed: true, paused: false, status_error: null }, host: { collector_state: 'new_unknown_state', database_state: 'ready', collector_dropped_lines: 0, reader_dropped_frames: 0, database_gap_events: 0, memory_dropped_notifications: 0, notify_session_active: false } } as unknown as Status;
  const markup = renderToStaticMarkup(createElement(HealthStrip, { status }));
  assert.match(markup.replace(/<[^>]+>/g, ''), /系统采集未知状态/);
  assert.ok(!markup.includes('正常'));
});

test('服务失败原因区分系统任务失败、宿主未确认和授权拒绝', () => {
  assert.ok(errorMessage('service_command_failed').includes('系统服务命令执行失败'));
  assert.ok(errorMessage('service_steps_failed').includes('查看具体步骤'));
  assert.ok(errorMessage('monitoring_state_unconfirmed').includes('管理宿主未确认'));
  assert.ok(errorMessage('service_result_invalid').includes('不能报告操作成功'));
  assert.ok(errorMessage('native_authorization_denied').includes('系统授权被拒绝'));
  assert.ok(errorMessage('native_authorization_unavailable').includes('系统授权窗口不可用'));
});

test('部分失败的服务操作保留具体步骤，不显示完成', () => {
  const markup = renderToStaticMarkup(createElement(SettingsPage, { active: false, operate: () => {}, operation: {
    id: 'synthetic-operation', operation: 'pause', state: 'failed', error: 'service_steps_failed',
    report: { data_preserved: true, steps: [{ label: 'collector', success: false, message: '采集任务卸载命令失败，需核对任务实际状态' }] },
  } }));
  assert.ok(markup.includes('失败：采集任务卸载命令失败，需核对任务实际状态'));
  assert.ok(!markup.includes('服务操作已完成'));
});

test('任务状态查询失败时显示未知，不把缺失结果写成停止或暂停成功', () => {
  const job = { loaded: false, running: false, disabled: true, last_exit_code: null };
  const status = { service: { installed: true, installed_binary_trusted: true, status_error: 'launchd_status_unavailable', collector: job, analyzer: job, notification: job, paused: true }, service_actions_enabled: false } as unknown as Status;
  const markup = renderToStaticMarkup(createElement(SettingsPage, { active: false, operate: () => {}, status }));
  assert.ok(!markup.includes('未加载，未运行'));
  assert.ok(!markup.includes('已暂停，跨重启保留'));
  assert.ok((markup.match(/<dd>未知<\/dd>/g) ?? []).length >= 4);
});
