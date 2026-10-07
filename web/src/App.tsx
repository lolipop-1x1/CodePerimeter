import { Trans } from 'react-i18next';
import { i18n, t, useLocale } from './i18n';
import { useEffect, useState } from 'react';
import { Button, GlobalTheme, MenuButton, MenuItemRadioGroup } from '@carbon/react';
import { Dashboard, Folder, Document, Archive, WarningAlt, Settings, Tools, Sun, Moon, Screen, Language } from '@carbon/react/icons';
import { api, errorMessage, initializeSession } from './api';
import { Notice } from './components';
import { DetailPane, type Selection } from './details';
import { DirectoriesPage } from './directories';
import { usePolling } from './hooks';
import { primaryServiceAction } from './format';
import { HealthStrip, OverviewPage, type RecordScope } from './overview';
import { RecordsPage } from './records';
import { RulesPage } from './rules';
import { serviceLabels, SettingsPage, type ServiceAction } from './settings';
import type { ServiceOperation, Status } from './types';
import { alertLocation, alertRoute } from './route';
import { useLanguagePreference } from './language';

const pages = [
  { key: 'overview', get label() { return t('app.overview'); }, get title() { return t('app.monitoringOverview'); }, get description() { return t('app.fileActivityAndArchiveIndicators'); }, icon: Dashboard },
  { key: 'directories', get label() { return t('app.monitoredDirectories'); }, get title() { return t('app.monitoredDirectories'); }, get description() { return t('app.projectScopeHistorySourcesAndSubtreeExclusions'); }, icon: Folder },
  { key: 'events', get label() { return t('app.fileActivity'); }, get title() { return t('app.fileActivity'); }, get description() { return t('app.observedEventsAndProcessIdentities'); }, icon: Document },
  { key: 'archives', get label() { return t('app.archiveIndicators'); }, get title() { return t('app.archiveIndicators'); }, get description() { return t('app.commandAndOutputBehaviorIndicators'); }, icon: Archive },
  { key: 'alerts', get label() { return t('app.alertCenter'); }, get title() { return t('app.alertCenter'); }, get description() { return t('app.readHandleAndTraceEvidence'); }, icon: WarningAlt },
  { key: 'rules', get label() { return t('app.ruleCenter'); }, get title() { return t('app.ruleCenter'); }, get description() { return t('app.builtInRulesAndGlobalParameters'); }, icon: Settings },
  { key: 'settings', get label() { return t('app.settingsAndDiagnostics'); }, get title() { return t('app.settingsAndDiagnostics'); }, get description() { return t('app.systemServicesDataAndCoverageGaps'); }, icon: Tools },
] as const;
const themeLabels: Record<string, string> = { get system() { return t('app.followSystem'); }, get light() { return t('app.light'); }, get dark() { return t('app.dark'); } };

export function App() {
  useLocale();
  const [authenticated, setAuthenticated] = useState(initializeSession);
  const language = useLanguagePreference(authenticated);
  const [page, setPage] = useState<string>(() => alertRoute(window.location.search).page);
  const [selection, setSelection] = useState<Selection>(() => { const id = alertRoute(window.location.search).alert; return id ? { kind: 'alert', id } : null; });
  const [routeError, setRouteError] = useState<string | undefined>(() => alertRoute(window.location.search).invalid ? 'app.theAlertLinkInThisNotificationIs' : undefined);
  const [recordScope, setRecordScope] = useState<RecordScope>();
  const [operation, setOperation] = useState<ServiceOperation>();
  const [operationError, setOperationError] = useState<string>();
  const [starting, setStarting] = useState(false);
  const [theme, setTheme] = useState(() => { try { const saved = localStorage.getItem('codeperimeter-theme'); return saved && themeLabels[saved] ? saved : 'system'; } catch { return 'system'; } });
  const [darkSystem, setDarkSystem] = useState(() => window.matchMedia('(prefers-color-scheme: dark)').matches);
  const dark = theme === 'dark' || (theme === 'system' && darkSystem);
  const status = usePolling<Status>('/api/status', undefined, authenticated);
  const operationState = usePolling<ServiceOperation>(`/api/service/${operation?.id ?? ''}`, undefined, authenticated && operation?.state === 'running');
  useEffect(() => { if (operationState.data) { setOperation(operationState.data); if (operationState.data.state === 'failed') setOperationError(operationState.data.error ?? 'api.theOperationDidNotCompleteCheckDiagnostics'); if (operationState.data.state !== 'running') status.refresh(); } }, [operationState.data]);
  useEffect(() => { if (operationState.errorCode) setOperationError(operationState.errorCode); }, [operationState.errorCode]);
  useEffect(() => {
    const expired = () => setAuthenticated(false);
    window.addEventListener('codeperimeter-session-expired', expired);
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    const changed = () => setDarkSystem(media.matches);
    media.addEventListener('change', changed);
    return () => { window.removeEventListener('codeperimeter-session-expired', expired); media.removeEventListener('change', changed); };
  }, []);
  useEffect(() => { document.documentElement.dataset.theme = dark ? 'dark' : 'light'; try { localStorage.setItem('codeperimeter-theme', theme); } catch { /* 无持久存储时沿用本次选择。 */ } }, [dark, theme]);
  useEffect(() => {
    const restore = () => {
      const route = alertRoute(window.location.search);
      setPage(route.page); setSelection(route.alert ? { kind: 'alert', id: route.alert } : null);
      setRouteError(route.invalid ? 'app.theAlertLinkInThisNotificationIs' : undefined);
    };
    window.addEventListener('popstate', restore);
    return () => window.removeEventListener('popstate', restore);
  }, []);
  async function operate(action: ServiceAction) {
    if (starting || operation?.state === 'running') return;
    setStarting(true); setOperationError(undefined);
    try { setOperation(await api<ServiceOperation>('/api/service', { operation: action })); }
    catch (error) { setOperationError(error instanceof Error ? error.message : t('app.unableToStartTheServiceOperation')); }
    finally { setStarting(false); }
  }
  const select = (next: Selection) => {
    setSelection(next); setRouteError(undefined);
    if (next?.kind === 'alert') setPage('alerts');
    history.pushState(null, '', alertLocation(window.location, next?.kind === 'alert' ? next.id : undefined, next === null && page === 'alerts'));
  };
  const navigate = (next: string) => { setPage(next); setSelection(null); setRouteError(undefined); history.replaceState(null, '', alertLocation(window.location, undefined, next === 'alerts')); };
  const navigateRecords = (scope?: Omit<RecordScope, 'key'>) => { setRecordScope({ ...scope, key: Date.now() }); navigate('events'); };
  const current = pages.find(item => item.key === page)!;
  const statusData = status.data;
  const mainAction = status.error ? null : primaryServiceAction(statusData?.service);
  const ThemeIcon = theme === 'system' ? Screen : theme === 'dark' ? Moon : Sun;
  return <GlobalTheme theme={dark ? 'g100' : 'g10'}><div className="console-shell">
    <a className="skip-link" href="#main-content">{t('app.skipToMainContent')}</a>
    <nav className="side-nav" aria-label={t('app.mainNavigation')}><div className="brand"><img className="brand-icon" src="/codeperimeter.svg" alt="" width="32" height="32" /><div><strong>CodePerimeter</strong><span>{t('app.localMonitoringConsole')}</span></div></div>
      <div className="nav-links">{pages.map(item => <button key={item.key} type="button" aria-current={page === item.key ? 'page' : undefined} onClick={() => navigate(item.key)}><item.icon size={20} /><span>{item.label}</span></button>)}</div>
      <div className="nav-bottom"><div className="theme-control"><Language className="theme-icon" size={18} aria-hidden="true" /><MenuButton className="theme-menu" label={t('language.menuLabel', { language: language.label })} kind="ghost" size="sm" disabled={!authenticated || language.saving} menuAlignment="top-start" menuBorder><MenuItemRadioGroup label={t('language.menuTitle')} items={['system', ...language.languages.map(item => item.id)]} itemToString={item => String(item) === 'system' ? t('language.followSystem') : language.languages.find(value => value.id === item)?.name ?? String(item)} selectedItem={language.preference} onChange={item => void language.select(String(item))} /></MenuButton></div><div className="theme-control"><ThemeIcon className="theme-icon" size={18} aria-hidden="true" /><MenuButton className="theme-menu" label={t('app.appearance', { mode: themeLabels[theme] })} kind="ghost" size="sm" menuAlignment="top-start" menuBorder><MenuItemRadioGroup label={t('app.appearanceMode')} items={['system', 'light', 'dark']} itemToString={item => themeLabels[String(item)]} selectedItem={theme} onChange={item => setTheme(String(item))} /></MenuButton></div></div>
    </nav>
    <main id="main-content" tabIndex={-1} className="main-content">
      <header className="page-heading"><div><h1>{current.title}</h1><p>{current.description}</p></div><div className="actions"><Button kind="tertiary" size="sm" disabled={!authenticated || !mainAction || !statusData?.service_actions_enabled || starting || operation?.state === 'running'} onClick={() => mainAction && void operate(mainAction)}>{starting || operation?.state === 'running' ? t('app.working') : mainAction ? serviceLabels[mainAction] : t('app.serviceStateUnknown')}</Button></div></header>
      {!authenticated ? <section className="panel entry-expired"><h2>{t('app.reopenTheConsole')}</h2><p><Trans i18n={i18n} i18nKey="app.reopenInstructions" values={{ command: 'codeperimeter ui' }} components={{ command: <code /> }} /></p><p className="helper">{t('app.theWebConsoleDoesNotReceiveAdministrator')}</p></section> : <>
        <Notice error={routeError ?? status.error ?? (operationError ? errorMessage(operationError) : undefined) ?? language.error} success={operation?.state === 'cancelled' ? t('app.theAdministratorOperationWasCancelledSuccessWas') : undefined} />
        {operation?.state === 'running' && <div className="operation-progress" role="status">{t('app.aServiceOperationIsInProgressCheck')}</div>}
        <HealthStrip status={statusData} />
        {statusData && !statusData.host && <section className="host-unavailable"><h2>{statusData.service.status_error ? t('app.backgroundServiceStateIsCurrentlyUnknown') : statusData.service.installed ? t('app.theManagementHostIsCurrentlyUnavailable') : t('app.backgroundServiceIsNotInstalled')}</h2><p>{statusData.host_error ? errorMessage(statusData.host_error) : t('app.historyQueriesAndMonitoringSettingsNeedThe')}</p><Button kind="ghost" size="sm" onClick={() => navigate('settings')}>{t('app.openSettingsAndDiagnostics')}</Button></section>}
        <div className={`workspace${selection ? ' has-detail' : ''}`}><div className="page-content">
          <section hidden={page !== 'overview'} aria-label={t('app.overview')}><OverviewPage active={page === 'overview'} select={select} navigateRecords={navigateRecords} /></section>
          <section hidden={page !== 'directories'} aria-label={t('app.monitoredDirectories')}><DirectoriesPage active={page === 'directories'} /></section>
          <section hidden={page !== 'events'} aria-label={t('app.fileActivity')}><RecordsPage active={page === 'events'} select={select} initialScope={recordScope} /></section>
          <section hidden={page !== 'archives'} aria-label={t('app.archiveIndicators')}><RecordsPage active={page === 'archives'} archive select={select} /></section>
          <section hidden={page !== 'alerts'} aria-label={t('app.alertCenter')}><RecordsPage active={page === 'alerts'} alerts select={select} /></section>
          <section hidden={page !== 'rules'} aria-label={t('app.ruleCenter')}><RulesPage active={page === 'rules'} /></section>
          <section hidden={page !== 'settings'} aria-label={t('app.settingsAndDiagnostics')}><SettingsPage active={page === 'settings'} status={statusData} operation={operation} operate={action => void operate(action)} operationError={operationError ? errorMessage(operationError) : undefined} /></section>
        </div><DetailPane selection={selection} select={select} close={() => select(null)} viewRule={() => navigate('rules')} viewProcess={pid => navigateRecords({ pid: String(pid) })} /></div>
        <footer>{t('app.fileActivityAndArchiveIndicatorsSupportDiscovery')}</footer>
      </>}
    </main>
  </div></GlobalTheme>;
}
