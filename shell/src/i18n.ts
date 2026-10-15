// i18n: i18next + react-i18next — the shell's standard i18n layer.
// Shell-owned strings live in the 'shell' namespace; module-tier
// components join the SAME instance (react is already a locked singleton
// via the import map — i18next follows the same sharing story) by
// registering their own namespaces with i18n.addResourceBundle, and
// locale switches propagate everywhere.
// Language detection: localStorage ('sl-locale') → system language;
// changes persist and keep <html lang> aligned.

import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';

export type Locale = 'zh' | 'en';

const STORAGE_KEY = 'sl-locale';

const zh = {
  // App shell / menu / activity bar
  'menu.components': '组件管理',
  'menu.settings': '设置',
  'app.compNotFound':
    '未找到命名空间为「{{ns}}」的已安装组件（可能已卸载，组件列表加载中或读取失败）',
  'title.configFallback': '组件配置',
  'theme.toDark': '深色',
  'theme.toLight': '浅色',

  // Welcome page
  'welcome.title': '欢迎使用 Superlive',
  'welcome.desc': '组件化的个人平台——左侧菜单进入已安装的组件，底部管理组件与查看设置。',

  // Component host (module-tier container)
  'host.loading': '组件加载中…',
  'host.renderFailed': '组件加载或渲染失败：{{error}}',

  // Settings page
  'settings.update': '应用更新',
  'settings.check': '检查更新',
  'settings.updateHint': '当前版本 {{version}}，检查并安装 Superlive 的最新版本。',
  'settings.language': '语言',
  'settings.languageHint': '界面语言，切换立即生效。',

  // Update dialog
  'update.checking': '正在检查更新…',
  'update.available': '发现新版本 {{version}}',
  'update.uptodate': '当前已是最新版本。',
  'update.downloading': '下载中…',
  'update.installed': '更新已安装，重启应用后生效。',
  'update.failed': '更新失败: {{error}}',
  'update.close': '关闭',
  'update.install': '下载并安装 {{version}}',

  // Component management page
  'state.needs-config': '待配置',
  'state.ready': '就绪',
  'state.running': '运行中',
  'state.stopped': '已停止',
  'mgr.installPlaceholder': '组件包 zip 的磁盘路径（也可把 zip 拖进窗口安装）',
  'mgr.install': '安装',
  'mgr.listFailed': '读取组件列表失败: {{error}}',
  'mgr.installedDrag': '已安装（拖拽）: {{path}}',
  'mgr.installed': '已安装: {{path}}',
  'mgr.actionFailed': '操作失败: {{error}}',
  'mgr.stateReadFailed': '读取组件状态失败: {{error}}',
  'mgr.started': '已启动: {{id}}',
  'mgr.startFailed': '启动失败: {{error}}',
  'mgr.stopRequested': '已请求停止: {{id}}',
  'mgr.uninstalled': '已卸载: {{id}}',
  'table.component': '组件',
  'table.version': '版本',
  'table.namespace': '命名空间',
  'table.ui': 'UI',
  'table.state': '状态',
  'table.actions': '操作',
  'mgr.empty': '尚未安装任何组件',
  'mgr.goConfig': '去配置',
  'mgr.configure': '配置',
  'mgr.stop': '停止',
  'mgr.start': '启动',
  'mgr.uninstall': '卸载',
  'mgr.uninstallConfirm': '卸载组件 {{id}}？（将停止进程并删除全部数据）',
  'mgr.uninstallConfirmDev': '卸载组件 {{id}}？（仅移除注册，项目目录保留）',
  'mgr.installedTitle': '组件已安装',
  'mgr.later': '稍后',
  'mgr.startNow': '立即启动',
  'mgr.installedPrompt':
    '组件「{{name}}」安装完成。是否立即启动？存在未完成必填配置的组件会先进入配置页。',

  // Launch options dialog
  'launch.title': '启动选项',
  'launch.later': '稍后再说',
  'launch.saveAndStart': '保存并启动',
  'launch.policy': '启动策略',
  'launch.auto': '自动：每次打开 Superlive 时自动拉起该组件',
  'launch.manual': '手动：在组件管理页手动启动',
  'launch.policyFailed': '保存启动策略失败: {{error}}',

  // Component creation dialog (the creation entry over the create_component
  // command; field-level errors branch on the command error's kind, messages
  // pass the command's original text through — the established convention)
  'mgr.create': '创建组件',
  'dlg.title': '创建组件',
  'dlg.idLabel': '组件 ID',
  'dlg.idHint': '仅限字母、数字与连字符',
  'dlg.idRequired': '组件 ID 必填',
  'dlg.titlesLabel': '多语言标题',
  'dlg.titlesLocale': '语言',
  'dlg.titlesValue': '标题',
  'dlg.titlesAdd': '添加语言',
  'dlg.titlesRemove': '移除此行',
  'dlg.titlesValueRequired': '必填标题',
  'dlg.titlesLocaleDuplicate': '语言重复',
  'dlg.titlesHint': '菜单与路由标题按语言显示；每行标题必填，至少保留一行。',
  'dlg.nsLabel': '命名空间',
  'dlg.nsHint': '仅限字母、数字与连字符；留空则与组件 ID 相同',
  'dlg.shapeLabel': '形态',
  'dlg.shapeEntry': '前端界面（entry）',
  'dlg.shapeLaunch': '后端进程（launch）',
  'dlg.shapeBoth': '前后端结合：Web 界面 + 后端进程',
  'dlg.shapeFrontend': '纯前端：仅 Web 界面，无后端进程',
  'dlg.shapeBackend': '纯后端：仅后端进程，无界面挂载',
  'dlg.shapeNone': '至少勾选一项能力后才能创建',
  'dlg.targetDirLabel': '项目位置',
  'dlg.targetDirPlaceholder': '/absolute/path/to/project',
  'dlg.targetDirHint': '目录须不存在，或为空目录。',
  'dlg.browse': '浏览…',
  'dlg.targetDirRequired': '项目位置必填',
  'dlg.advanced': '高级选项',
  'dlg.devPortLabel': '开发端口',
  'dlg.portInvalid': '端口须为 1-65535 的整数',
  'dlg.sdkModeLabel': 'SDK 依赖',
  'dlg.sdkRegistry': 'registry：使用 crates.io / npm 的发布版本',
  'dlg.sdkLocal': 'local：指向 superlive 本地检出（path 依赖）',
  'dlg.sdkPathLabel': 'superlive 检出路径',
  'dlg.sdkPathPlaceholder': 'superlive 仓库的本地检出目录',
  'dlg.sdkPathRequired': '须填写检出路径',
  'dlg.cancel': '取消',
  'dlg.create': '创建',
  'dlg.creating': '创建中…',
  'dlg.invalid': '参数无效：{{message}}',
  'dlg.conflict': '无法创建：{{message}}',
  'dlg.createFailed': '创建失败：{{message}}',

  // Import flow (the creation entry's first step: import an existing project)
  'dlg.card.importTitle': '导入已有项目',
  'dlg.card.importDesc': '已有组件项目？选择目录直接注册为开发组件。',
  'dlg.card.freshTitle': '全新创建',
  'dlg.card.freshDesc': '从脚手架生成一个新的组件项目。',
  'dlg.import.stepTitle': '导入已有项目',
  'dlg.import.dirLabel': '项目目录',
  'dlg.import.dirPlaceholder': '/absolute/path/to/project',
  'dlg.import.dirHint': '导入不改动项目文件；缺失的脚手架文件会先列出征求确认。',
  'dlg.import.inspect': '检查并导入',
  'dlg.import.checking': '检查中…',
  'dlg.back': '返回',
  'dlg.import.upgradeTitle': '发现可补齐项',
  'dlg.import.upgradeHint': '确认后将应用以下修正并注册组件；已有文件不会被覆盖。',
  'dlg.import.apply': '应用并注册',
  'dlg.import.applying': '应用中…',
  'dlg.import.rejected': '无法导入：{{message}}',
  'dlg.import.failed': '导入失败：{{message}}',
  'dlg.up.deriveTitles': '补齐菜单与路由标题',
  'dlg.up.refreshVersion': '刷新版本号',
  'dlg.up.fillMinShellVersion': '填充最低壳版本',
  'dlg.up.addBuildScript': '生成 scripts/superlive.build',
  'dlg.up.addReadme': '生成 README.md',

  // Post-create guide (the create flow's tail: project path + next steps)
  'done.title': '创建完成',
  'done.intro': '组件「{{name}}」已创建并注册为开发（dev）组件。',
  'done.projectDir': '项目位置',
  'done.nextSteps': '后续步骤',
  'done.locationNote': 'dev 组件的主体保留在项目目录：改动代码后重载即升级，无需重新安装。',
  'done.close': '完成',

  // Component config page (shell-side texts; manifest labels go through the component's own i18n)
  'cfg.loadFailed': '读取组件配置声明失败: {{error}}',
  'cfg.nan': '须为数字',
  'cfg.savedRestart': '已保存。涉及需重启的配置项，组件已由壳自动重启。',
  'cfg.saved': '已保存',
  'cfg.savedHot': '已保存，配置已热重载生效。',
  'cfg.savedStopped': '已保存，组件未运行。',
  'cfg.saveFailed': '保存配置失败: {{error}}',
  'cfg.required': '必填',
  'cfg.restartTag': '重启生效',
  'cfg.unset': '（未设置）',
  'cfg.enabled': '启用',
  'cfg.default': '默认：{{value}}',
  'cfg.notSet': '未设置',
  'cfg.hide': '隐藏',
  'cfg.show': '显示',
  'cfg.loading': '加载配置声明中…',
  'cfg.back': '← 返回组件管理',
  'cfg.title': '配置：{{id}}',
  'cfg.noFields': '该组件没有可配置项（零配置组件，可直接启动）。',
  'cfg.save': '保存配置',
} as const;

export type ShellKey = keyof typeof zh;

// Parity is compile-enforced: a missing en key fails the build
const en: Record<ShellKey, string> = {
  // App shell / menu / activity bar
  'menu.components': 'Components',
  'menu.settings': 'Settings',
  'app.compNotFound':
    'No installed component found for namespace "{{ns}}" (it may have been uninstalled, or the component list is still loading or failed to load)',
  'title.configFallback': 'Configuration',
  'theme.toDark': 'Dark',
  'theme.toLight': 'Light',

  // Welcome page
  'welcome.title': 'Welcome to Superlive',
  'welcome.desc':
    'A component-based personal platform — open installed components from the menu above; manage components and settings at its foot.',

  // Component host (module-tier container)
  'host.loading': 'Loading component…',
  'host.renderFailed': 'Component failed to load or render: {{error}}',

  // Settings page
  'settings.update': 'App update',
  'settings.check': 'Check for updates',
  'settings.updateHint': 'Current version {{version}}. Check for and install the latest Superlive release.',
  'settings.language': 'Language',
  'settings.languageHint': 'Interface language — applies immediately.',

  // Update dialog
  'update.checking': 'Checking for updates…',
  'update.available': 'New version {{version}} available',
  'update.uptodate': "You're on the latest version.",
  'update.downloading': 'Downloading…',
  'update.installed': 'Update installed. Restart the app to apply it.',
  'update.failed': 'Update failed: {{error}}',
  'update.close': 'Close',
  'update.install': 'Download and install {{version}}',

  // Component management page
  'state.needs-config': 'Needs config',
  'state.ready': 'Ready',
  'state.running': 'Running',
  'state.stopped': 'Stopped',
  'mgr.installPlaceholder': 'Disk path of a component zip (or drag a zip into the window)',
  'mgr.install': 'Install',
  'mgr.listFailed': 'Failed to read the component list: {{error}}',
  'mgr.installedDrag': 'Installed (dragged): {{path}}',
  'mgr.installed': 'Installed: {{path}}',
  'mgr.actionFailed': 'Action failed: {{error}}',
  'mgr.stateReadFailed': 'Failed to read component state: {{error}}',
  'mgr.started': 'Started: {{id}}',
  'mgr.startFailed': 'Start failed: {{error}}',
  'mgr.stopRequested': 'Stop requested: {{id}}',
  'mgr.uninstalled': 'Uninstalled: {{id}}',
  'table.component': 'Component',
  'table.version': 'Version',
  'table.namespace': 'Namespace',
  'table.ui': 'UI',
  'table.state': 'State',
  'table.actions': 'Actions',
  'mgr.empty': 'No components installed yet',
  'mgr.goConfig': 'Set up',
  'mgr.configure': 'Config',
  'mgr.stop': 'Stop',
  'mgr.start': 'Start',
  'mgr.uninstall': 'Uninstall',
  'mgr.uninstallConfirm': 'Uninstall {{id}}? (stops the process and deletes all its data)',
  'mgr.uninstallConfirmDev':
    'Uninstall {{id}}? (removes the registration only; the project directory stays untouched)',
  'mgr.installedTitle': 'Component installed',
  'mgr.later': 'Later',
  'mgr.startNow': 'Start now',
  'mgr.installedPrompt':
    '"{{name}}" installed. Start it now? Components with unfinished required configuration go to the configuration page first.',

  // Launch options dialog
  'launch.title': 'Launch options',
  'launch.later': 'Not now',
  'launch.saveAndStart': 'Save and start',
  'launch.policy': 'Launch policy',
  'launch.auto': 'Auto: start this component automatically whenever Superlive opens',
  'launch.manual': 'Manual: start it yourself from the component management page',
  'launch.policyFailed': 'Failed to save the launch policy: {{error}}',

  // Component creation dialog
  'mgr.create': 'Create component',
  'dlg.title': 'Create component',
  'dlg.idLabel': 'Component ID',
  'dlg.idHint': 'Letters, digits, and hyphens only',
  'dlg.idRequired': 'Component ID is required',
  'dlg.titlesLabel': 'Localized titles',
  'dlg.titlesLocale': 'Locale',
  'dlg.titlesValue': 'Title',
  'dlg.titlesAdd': 'Add locale',
  'dlg.titlesRemove': 'Remove this row',
  'dlg.titlesValueRequired': 'Title is required',
  'dlg.titlesLocaleDuplicate': 'Duplicate locale',
  'dlg.titlesHint':
    'Menu and route titles are shown per locale; every row needs a title — keep at least one row.',
  'dlg.nsLabel': 'Namespace',
  'dlg.nsHint': 'Letters, digits, and hyphens only; defaults to the component ID when empty',
  'dlg.shapeLabel': 'Shape',
  'dlg.shapeEntry': 'Frontend UI (entry)',
  'dlg.shapeLaunch': 'Backend process (launch)',
  'dlg.shapeBoth': 'Frontend + backend: a web UI and a backend process',
  'dlg.shapeFrontend': 'Frontend only: a web UI, no backend process',
  'dlg.shapeBackend': 'Backend only: a backend process, no UI mount surface',
  'dlg.shapeNone': 'Check at least one capability to create',
  'dlg.targetDirLabel': 'Project location',
  'dlg.targetDirPlaceholder': '/absolute/path/to/project',
  'dlg.targetDirHint': 'Must not exist, or be an empty directory.',
  'dlg.browse': 'Browse…',
  'dlg.targetDirRequired': 'Project location is required',
  'dlg.advanced': 'Advanced',
  'dlg.devPortLabel': 'Dev port',
  'dlg.portInvalid': 'The port must be an integer between 1 and 65535',
  'dlg.sdkModeLabel': 'SDK dependencies',
  'dlg.sdkRegistry': 'registry: published crates.io / npm versions',
  'dlg.sdkLocal': 'local: a superlive checkout (path dependencies)',
  'dlg.sdkPathLabel': 'superlive checkout path',
  'dlg.sdkPathPlaceholder': 'Local checkout directory of the superlive repository',
  'dlg.sdkPathRequired': 'SDK path is required',
  'dlg.cancel': 'Cancel',
  'dlg.create': 'Create',
  'dlg.creating': 'Creating…',
  'dlg.invalid': 'Invalid argument: {{message}}',
  'dlg.conflict': 'Cannot create: {{message}}',
  'dlg.createFailed': 'Creation failed: {{message}}',

  // Import flow (the creation entry's first step: import an existing project)
  'dlg.card.importTitle': 'Import an existing project',
  'dlg.card.importDesc':
    'Already have a component project? Pick its directory to register it as a dev component.',
  'dlg.card.freshTitle': 'Create fresh',
  'dlg.card.freshDesc': 'Scaffold a brand-new component project.',
  'dlg.import.stepTitle': 'Import an existing project',
  'dlg.import.dirLabel': 'Project directory',
  'dlg.import.dirPlaceholder': '/absolute/path/to/project',
  'dlg.import.dirHint':
    'Importing never modifies project files; missing scaffold files are listed for confirmation first.',
  'dlg.import.inspect': 'Inspect & import',
  'dlg.import.checking': 'Inspecting…',
  'dlg.back': 'Back',
  'dlg.import.upgradeTitle': 'Upgradeable findings',
  'dlg.import.upgradeHint':
    'Confirming applies these fixes and registers the component; existing files are never overwritten.',
  'dlg.import.apply': 'Apply & register',
  'dlg.import.applying': 'Applying…',
  'dlg.import.rejected': 'Cannot import: {{message}}',
  'dlg.import.failed': 'Import failed: {{message}}',
  'dlg.up.deriveTitles': 'Derive menu & route titles',
  'dlg.up.refreshVersion': 'Refresh the version',
  'dlg.up.fillMinShellVersion': 'Fill the minimum shell version',
  'dlg.up.addBuildScript': 'Generate scripts/superlive.build',
  'dlg.up.addReadme': 'Generate README.md',

  // Post-create guide
  'done.title': 'Component created',
  'done.intro': '"{{name}}" has been created and registered as a dev component.',
  'done.projectDir': 'Project location',
  'done.nextSteps': 'Next steps',
  'done.locationNote':
    "A dev component's body stays in the project directory: reload after changes to upgrade — no reinstall needed.",
  'done.close': 'Done',

  // Component config page
  'cfg.loadFailed': 'Failed to read the component configuration declaration: {{error}}',
  'cfg.nan': 'Must be a number',
  'cfg.savedRestart':
    'Saved. Entries requiring a restart have been applied — the shell restarted the component automatically.',
  'cfg.saved': 'Saved',
  'cfg.savedHot': 'Saved. The configuration was hot-reloaded.',
  'cfg.savedStopped': 'Saved. The component is not running.',
  'cfg.saveFailed': 'Failed to save the configuration: {{error}}',
  'cfg.required': 'Required',
  'cfg.restartTag': 'Restart required',
  'cfg.unset': '(not set)',
  'cfg.enabled': 'Enabled',
  'cfg.default': 'Default: {{value}}',
  'cfg.notSet': 'Not set',
  'cfg.hide': 'Hide',
  'cfg.show': 'Show',
  'cfg.loading': 'Loading configuration…',
  'cfg.back': '← Back to components',
  'cfg.title': 'Configuration: {{id}}',
  'cfg.noFields': 'This component has no configurable entries (zero-config; it can start directly).',
  'cfg.save': 'Save configuration',
};

// Typed keys for t() call sites (react-i18next CustomTypeOptions)
declare module 'i18next' {
  interface CustomTypeOptions {
    defaultNS: 'shell';
    resources: {
      shell: typeof zh;
    };
  }
}

function detectInitialLocale(): Locale {
  const saved = localStorage.getItem(STORAGE_KEY);
  if (saved === 'zh' || saved === 'en') return saved;
  return navigator.language.toLowerCase().startsWith('zh') ? 'zh' : 'en';
}

void i18n.use(initReactI18next).init({
  lng: detectInitialLocale(),
  fallbackLng: 'en',
  defaultNS: 'shell',
  resources: {
    zh: { shell: zh },
    en: { shell: en },
  },
  // React already escapes rendered values
  interpolation: { escapeValue: false },
});

// Persist the choice and keep <html lang> aligned on every change
function applyLangSideEffects(lng: string): void {
  localStorage.setItem(STORAGE_KEY, lng);
  document.documentElement.lang = lng === 'zh' ? 'zh-CN' : 'en';
}
applyLangSideEffects(i18n.language);
i18n.on('languageChanged', applyLangSideEffects);

/** Locale switch (settings page). */
export function setLocale(locale: Locale): void {
  void i18n.changeLanguage(locale);
}

/**
 * Component-namespace menu-title lookup (architecture.md "Localized titles"):
 * resolves the fixed `menuTitle` key inside the component's own namespace —
 * the shell injects it from the manifest's titles at list-load time, seeding
 * both the zh and en bundles with the manifest `title` fallback value.
 * Fallback chain per lookup: requested locale → zh → en (the host's own
 * global fallback configuration stays untouched). Returns null when the
 * namespace carries no menuTitle resource (a component without a menu
 * registration), so callers can fall back to display_name.
 *
 * Component namespaces are dynamic data outside the shell's typed-key surface
 * (CustomTypeOptions covers the `shell` namespace only), so this helper is the
 * one deliberate untyped escape hatch — all other call sites keep compile-
 * enforced keys.
 */
export function componentMenuTitle(namespace: string): string | null {
  const exists = i18n.exists.bind(i18n) as (
    key: string,
    options?: Record<string, unknown>,
  ) => boolean;
  if (!exists('menuTitle', { ns: namespace })) return null;
  const t = i18n.t.bind(i18n) as (
    key: string,
    options?: Record<string, unknown>,
  ) => string;
  return t('menuTitle', { ns: namespace, fallbackLng: ['zh', 'en'] });
}

/**
 * Menu-title resource injection (architecture.md "Localized titles"): lands
 * the manifest's localized titles into the component's own namespace under
 * the fixed `menuTitle` key, beside the component's own registered resources
 * (one namespace holds everything the component surfaces to the host, so
 * cross-component key collisions are structurally impossible). The manifest's
 * single-language `title` fallback is additionally seeded into the zh and en
 * bundles so the fallback chain always terminates in a real value. Injection
 * is deep + overwrite and idempotent per list refresh; registered namespaces
 * stay for the session (no unload cycle).
 */
export function injectMenuTitles(
  namespace: string,
  titles: Record<string, string>,
  fallbackTitle: string,
): void {
  for (const [locale, value] of Object.entries(titles)) {
    i18n.addResourceBundle(locale, namespace, { menuTitle: value }, true, true);
  }
  const fallback = { menuTitle: fallbackTitle };
  i18n.addResourceBundle('zh', namespace, fallback, true, true);
  i18n.addResourceBundle('en', namespace, fallback, true, true);
}

export default i18n;
