import { useEffect, useState } from 'react';
import { Button, GlobalTheme, Select, SelectItem, Tag } from '@carbon/react';
import { Dashboard, Folder, Document, Archive, WarningAlt, Settings, Tools } from '@carbon/react/icons';
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

const pages = [
  { key: 'overview', label: '概览', title: '监控概览', description: '文件活动与归档线索', icon: Dashboard },
  { key: 'directories', label: '监控目录', title: '监控目录', description: '项目范围、历史来源与子树排除', icon: Folder },
  { key: 'events', label: '文件活动', title: '文件活动', description: '真实事件记录与进程身份', icon: Document },
  { key: 'archives', label: '归档迹象', title: '归档迹象', description: '命令与输出行为线索', icon: Archive },
  { key: 'alerts', label: '告警中心', title: '告警中心', description: '阅读、处理及证据追溯', icon: WarningAlt },
  { key: 'rules', label: '规则中心', title: '规则中心', description: '内置规则开关与全局参数', icon: Settings },
  { key: 'settings', label: '设置与诊断', title: '设置与诊断', description: '系统服务、数据管理与覆盖缺口', icon: Tools },
] as const;

export function App() {
  const [authenticated, setAuthenticated] = useState(initializeSession);
  const [page, setPage] = useState<string>('overview');
  const [selection, setSelection] = useState<Selection>(null);
  const [recordScope, setRecordScope] = useState<RecordScope>();
  const [operation, setOperation] = useState<ServiceOperation>();
  const [operationError, setOperationError] = useState<string>();
  const [starting, setStarting] = useState(false);
  const [theme, setTheme] = useState(() => { try { return localStorage.getItem('codeperimeter-theme') ?? 'system'; } catch { return 'system'; } });
  const [darkSystem, setDarkSystem] = useState(() => window.matchMedia('(prefers-color-scheme: dark)').matches);
  const dark = theme === 'dark' || (theme === 'system' && darkSystem);
  const status = usePolling<Status>('/api/status', undefined, authenticated);
  const operationState = usePolling<ServiceOperation>(`/api/service/${operation?.id ?? ''}`, undefined, authenticated && operation?.state === 'running');
  useEffect(() => { if (operationState.data) { setOperation(operationState.data); if (operationState.data.state === 'failed') setOperationError(errorMessage(operationState.data.error)); if (operationState.data.state !== 'running') status.refresh(); } }, [operationState.data]);
  useEffect(() => { if (operationState.error) setOperationError(operationState.error); }, [operationState.error]);
  useEffect(() => {
    const expired = () => setAuthenticated(false);
    window.addEventListener('codeperimeter-session-expired', expired);
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    const changed = () => setDarkSystem(media.matches);
    media.addEventListener('change', changed);
    return () => { window.removeEventListener('codeperimeter-session-expired', expired); media.removeEventListener('change', changed); };
  }, []);
  useEffect(() => { document.documentElement.dataset.theme = dark ? 'dark' : 'light'; try { localStorage.setItem('codeperimeter-theme', theme); } catch { /* 无持久存储时沿用本次选择。 */ } }, [dark, theme]);
  async function operate(action: ServiceAction) {
    if (starting || operation?.state === 'running') return;
    setStarting(true); setOperationError(undefined);
    try { setOperation(await api<ServiceOperation>('/api/service', { operation: action })); }
    catch (error) { setOperationError(error instanceof Error ? error.message : '无法启动服务操作。'); }
    finally { setStarting(false); }
  }
  const navigate = (next: string) => { setPage(next); setSelection(null); };
  const navigateRecords = (scope?: Omit<RecordScope, 'key'>) => { setRecordScope({ ...scope, key: Date.now() }); navigate('events'); };
  const current = pages.find(item => item.key === page)!;
  const statusData = status.data;
  const mainAction = status.error ? null : primaryServiceAction(statusData?.service);
  return <GlobalTheme theme={dark ? 'g100' : 'g10'}><div className="console-shell">
    <a className="skip-link" href="#main-content">跳到主要内容</a>
    <nav className="side-nav" aria-label="主要导航"><div className="brand"><strong>CodePerimeter</strong><span>本机监控控制台</span></div>
      <div className="nav-links">{pages.map(item => <button key={item.key} type="button" aria-current={page === item.key ? 'page' : undefined} onClick={() => navigate(item.key)}><item.icon size={20} /><span>{item.label}</span></button>)}</div>
      <div className="nav-bottom"><span>仅本机访问</span><Select id="theme-selection" labelText="外观" size="sm" value={theme} onChange={event => setTheme(event.target.value)}><SelectItem value="system" text="跟随系统" /><SelectItem value="light" text="浅色" /><SelectItem value="dark" text="深色" /></Select></div>
    </nav>
    <main id="main-content" tabIndex={-1} className="main-content">
      <header className="page-heading"><div><h1>{current.title}</h1><p>{current.description}</p></div><div className="actions"><Button kind="tertiary" size="sm" disabled={!authenticated || !mainAction || !statusData?.service_actions_enabled || starting || operation?.state === 'running'} onClick={() => mainAction && void operate(mainAction)}>{starting || operation?.state === 'running' ? '正在操作' : mainAction ? serviceLabels[mainAction] : '服务状态未知'}</Button><Tag type="gray">仅本机访问</Tag></div></header>
      {!authenticated ? <section className="panel entry-expired"><h2>请重新打开控制台</h2><p>当前标签页缺少有效入口。请在本机终端运行 <code>codeperimeter ui</code>，通过新打开的页面继续使用。</p><p className="helper">网页不接收管理员密码。</p></section> : <>
        <Notice error={status.error ?? operationError} success={operation?.state === 'cancelled' ? '管理员操作已取消，未报告成功。' : undefined} />
        {operation?.state === 'running' && <div className="operation-progress" role="status">服务操作进行中，请查看系统授权窗口；也可在系统窗口取消。</div>}
        <HealthStrip status={statusData} />
        {statusData && !statusData.host && <section className="host-unavailable"><h2>{statusData.service.status_error ? '后台服务状态暂时未知' : statusData.service.installed ? '管理宿主暂时不可用' : '后台服务尚未安装'}</h2><p>{statusData.host_error ? errorMessage(statusData.host_error) : '历史查询和监控配置需要管理宿主运行。可以继续查看实际安装状态与恢复入口。'}</p><Button kind="ghost" size="sm" onClick={() => navigate('settings')}>查看设置与诊断</Button></section>}
        <div className={`workspace${selection ? ' has-detail' : ''}`}><div className="page-content">
          <section hidden={page !== 'overview'} aria-label="概览"><OverviewPage active={page === 'overview'} select={setSelection} navigateRecords={navigateRecords} /></section>
          <section hidden={page !== 'directories'} aria-label="监控目录"><DirectoriesPage active={page === 'directories'} /></section>
          <section hidden={page !== 'events'} aria-label="文件活动"><RecordsPage active={page === 'events'} select={setSelection} initialScope={recordScope} /></section>
          <section hidden={page !== 'archives'} aria-label="归档迹象"><RecordsPage active={page === 'archives'} archive select={setSelection} /></section>
          <section hidden={page !== 'alerts'} aria-label="告警中心"><RecordsPage active={page === 'alerts'} alerts select={setSelection} /></section>
          <section hidden={page !== 'rules'} aria-label="规则中心"><RulesPage active={page === 'rules'} /></section>
          <section hidden={page !== 'settings'} aria-label="设置与诊断"><SettingsPage active={page === 'settings'} status={statusData} operation={operation} operate={action => void operate(action)} operationError={operationError} /></section>
        </div><DetailPane selection={selection} select={setSelection} close={() => setSelection(null)} viewRule={() => navigate('rules')} viewProcess={pid => navigateRecords({ pid: String(pid) })} /></div>
        <footer>文件活动和归档迹象用于发现与追溯；当前观察能力不执行外传拦截。</footer>
      </>}
    </main>
  </div></GlobalTheme>;
}
