const SESSION_KEY = 'codeperimeter-entry';
let entryToken = '';

export function initializeSession(): boolean {
  const fragment = new URLSearchParams(window.location.hash.slice(1));
  const incoming = fragment.get('token');
  if (incoming) {
    entryToken = incoming;
    try { sessionStorage.setItem(SESSION_KEY, incoming); } catch { /* 隐私模式可使用本次内存会话。 */ }
  } else {
    try { entryToken = sessionStorage.getItem(SESSION_KEY) ?? ''; } catch { entryToken = ''; }
  }
  if (window.location.hash) history.replaceState(null, '', window.location.pathname + window.location.search);
  return Boolean(entryToken);
}

export function clearSession() {
  entryToken = '';
  try { sessionStorage.removeItem(SESSION_KEY); } catch { /* 只清理当前标签页，不记录凭证。 */ }
  window.dispatchEvent(new Event('codeperimeter-session-expired'));
}

export class ApiError extends Error {
  status: number;
  constructor(message: string, status = 0) { super(message); this.status = status; }
}

export function errorMessage(code?: string | null): string {
  if (!code) return '操作未完成，请查看诊断状态。';
  const messages: Record<string, string> = {
    entry_expired: '控制台入口已失效，请重新运行 codeperimeter ui。',
    alert_unavailable: '这条告警明细暂不可用，可能已过期、被清除或尚未保存。请关闭详情查看告警列表。',
    origin_rejected: '浏览器请求来源校验失败，请使用当前本机入口，并检查是否有扩展修改请求。',
    origin_required: '浏览器没有提供有效的同源信息，请使用当前本机入口。',
    host_rejected: '入口地址与当前控制台不一致，请重新运行 codeperimeter ui。',
    host_unavailable: '管理宿主不可用，查询和配置暂时无法完成，请查看服务状态。',
    service_status_unavailable: '系统服务状态暂时无法查询，安装和运行状态保留未知。',
    request_invalid: '请求参数无效，请检查输入后重试。',
    operation_rejected: '此操作不能从网页执行。',
    directory_unavailable: '目录不存在、无法访问或不是目录，请重新选择。',
    history_unavailable: '无法发现历史目录，请稍后重试或手动添加。',
    random_unavailable: '本机系统无法创建安全会话，请重新打开控制台。',
    selection_invalid: '请选择有效数量的候选目录。',
    preview_expired: '候选预览已失效，请重新预览并选择。',
    selection_unavailable: '选择的候选当前不可用，请重新预览。',
    platform_unsupported: '此系统不支持当前原生操作。',
    native_operation_busy: '已有系统窗口或授权操作进行中，请先完成或取消。',
    directory_picker_failed: '系统目录选择未完成，请重试或手动输入。',
    service_operation_failed: '系统服务操作失败，请查看诊断和授权结果。',
    service_command_failed: '系统服务命令执行失败，请核查任务是否已停止或加载，再重试。',
    service_steps_failed: '系统服务有步骤未完成，请在设置与诊断中查看具体步骤，并核对实际状态。',
    monitoring_state_unconfirmed: '系统任务操作已完成，但管理宿主未确认暂停／恢复状态，请核查状态后重试。',
    service_result_invalid: '系统服务返回结果无法核验，不能报告操作成功。',
    native_authorization_denied: 'macOS 系统授权被拒绝，服务操作未完成。',
    native_authorization_unavailable: 'macOS 系统授权窗口不可用，服务操作未完成。',
    operation_unknown: '服务操作记录已失效，请重新检查实际状态。',
    export_invalid: '导出参数无效，请检查格式与筛选。',
    service_not_installed: '后台服务尚未安装，请先完成安装。',
    authorization_cancelled: '系统授权已取消。',
    authorization_denied: '系统授权未通过，请重新确认。',
    permission_denied: '系统权限不足，请查看完全磁盘访问授权指引。',
  };
  if (messages[code]) return messages[code];
  return /^[a-z0-9_]+$/.test(code) ? '操作未完成，请查看诊断状态并重试。' : code;
}

async function request(path: string, body?: unknown, signal?: AbortSignal): Promise<Response> {
  const response = await fetch(path, {
    method: body === undefined ? 'GET' : 'POST',
    headers: { Authorization: `Bearer ${entryToken}`, ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal,
    credentials: 'omit',
    cache: 'no-store',
  });
  if (response.status === 401) {
    clearSession();
    throw new ApiError('控制台入口已失效，请重新运行 codeperimeter ui。', 401);
  }
  return response;
}

export async function api<T>(path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
  let response: Response;
  try { response = await request(path, body, signal); } catch (error) {
    if (error instanceof ApiError || (error instanceof DOMException && error.name === 'AbortError')) throw error;
    throw new ApiError('本机控制台连接中断，请检查后台服务。');
  }
  let envelope: { ok: boolean; data: T; error?: string | null };
  try { envelope = await response.json(); } catch { throw new ApiError('接口响应无效，请重新打开控制台。', response.status); }
  if (!response.ok || !envelope.ok) throw new ApiError(errorMessage(envelope.error), response.status);
  return envelope.data;
}

export function consoleApi<T>(action: string, payload?: unknown, signal?: AbortSignal): Promise<T> {
  return api('/api/console', { action, ...(payload === undefined ? {} : { payload }) }, signal);
}

export function controlApi<T>(operation: string, payload?: unknown): Promise<T> {
  return api('/api/control', { operation, ...(payload === undefined ? {} : { payload }) });
}

export async function downloadExport(body: unknown) {
  const response = await request('/api/export', body);
  if (!response.ok) {
    const envelope = await response.json().catch(() => null);
    throw new ApiError(errorMessage(envelope?.error));
  }
  const blob = await response.blob();
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement('a');
  anchor.href = url;
  const disposition = response.headers.get('Content-Disposition') ?? '';
  anchor.download = disposition.match(/filename="?([\w.-]+)"?/)?.[1] ?? 'codeperimeter-export';
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
