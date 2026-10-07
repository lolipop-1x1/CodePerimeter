import { i18n, t } from './i18n.ts';
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
  if (!code) return t('api.theOperationDidNotCompleteCheckDiagnostics');
  if (i18n.exists(code)) return t(code);
  const messages: Record<string, string> = {
    entry_expired: t('api.thisConsoleEntryExpiredRunCodeperimeterUi'),
    alert_unavailable: t('api.alertDetailsAreUnavailableTheyMayHave'),
    origin_rejected: t('api.browserOriginValidationFailedUseTheCurrent'),
    origin_required: t('api.theBrowserDidNotProvideValidSame'),
    host_rejected: t('api.thisAddressDoesNotMatchTheCurrent'),
    host_unavailable: t('api.theManagementHostIsUnavailableQueriesAnd'),
    service_status_unavailable: t('api.systemServiceStatusCannotBeQueriedInstallation'),
    request_invalid: t('api.invalidRequestParametersCheckYourInputAnd'),
    operation_rejected: t('api.thisOperationIsUnavailableFromTheWeb'),
    directory_unavailable: t('api.theDirectoryIsMissingInaccessibleOrNot'),
    history_unavailable: t('api.historyDirectoriesCouldNotBeDiscoveredRetry'),
    random_unavailable: t('api.theLocalSystemCouldNotCreateA'),
    selection_invalid: t('api.selectAValidNumberOfCandidateDirectories'),
    preview_expired: t('api.theCandidatePreviewExpiredPreviewAndSelect'),
    selection_unavailable: t('api.selectedCandidatesAreUnavailablePreviewAgain'),
    platform_unsupported: t('api.thisSystemDoesNotSupportTheNative'),
    native_operation_busy: t('api.aSystemWindowOrAuthorizationIsAlready'),
    directory_picker_failed: t('api.systemDirectorySelectionDidNotCompleteRetry'),
    service_operation_failed: t('api.theSystemServiceOperationFailedCheckDiagnostics'),
    service_command_failed: t('api.theSystemServiceCommandFailedVerifyWhether'),
    service_steps_failed: t('api.someServiceStepsDidNotCompleteInspect'),
    monitoring_state_unconfirmed: t('api.systemJobOperationsCompletedButTheManagement'),
    service_result_invalid: t('api.theServiceResultCannotBeVerifiedSuccess'),
    native_authorization_denied: t('api.macosAuthorizationWasDeniedTheServiceOperation'),
    native_authorization_unavailable: t('api.theMacOSAuthorizationWindowIsUnavailableThe'),
    operation_unknown: t('api.theServiceOperationRecordExpiredCheckActual'),
    export_invalid: t('api.invalidExportParametersCheckFormatAndFilters'),
    service_not_installed: t('api.backgroundServiceIsNotInstalledInstallIt'),
    authorization_cancelled: t('api.systemAuthorizationCancelled'),
    authorization_denied: t('api.systemAuthorizationFailedConfirmAgain'),
    permission_denied: t('api.systemPermissionIsInsufficientCheckFullDisk'),
    invalid_language_preference: t('language.invalidPreference'),
    language_load_failed: t('language.loadFailed'),
    language_save_failed: t('language.saveFailed'),
  };
  if (messages[code]) return messages[code];
  return /^[a-z0-9_]+$/.test(code) ? t('api.theOperationDidNotCompleteCheckDiagnostics2') : t('common.originalText', { text: code });
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
    throw new ApiError('entry_expired', 401);
  }
  return response;
}

export async function api<T>(path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
  let response: Response;
  try { response = await request(path, body, signal); } catch (error) {
    if (error instanceof ApiError || (error instanceof DOMException && error.name === 'AbortError')) throw error;
    throw new ApiError('api.theLocalConsoleConnectionWasInterruptedCheck');
  }
  let envelope: { ok: boolean; data: T; error?: string | null };
  try { envelope = await response.json(); } catch { throw new ApiError('api.invalidInterfaceResponseReopenTheConsole', response.status); }
  if (!response.ok || !envelope.ok) throw new ApiError(envelope.error ?? 'api.theOperationDidNotCompleteCheckDiagnostics', response.status);
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
    throw new ApiError(envelope?.error ?? 'api.theOperationDidNotCompleteCheckDiagnostics');
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
