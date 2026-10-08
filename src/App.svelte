<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { getCurrentWindow } from '@tauri-apps/api/window';

  const appWindow = getCurrentWindow();

  type Session = { day: string; startedAt: string; activeSeconds: number; endedAt: string | null };
  type WeekDay = { date: string; label: string; seconds: number };
  type Snapshot = {
    today: string; startedAt: string; activeSeconds: number; todaySeconds: number;
    idleSeconds: number; activeNow: boolean; goalHours: number; goalSeconds: number;
    notifyGoal: boolean; pauseMinutes: number; layoutType: string; theme: string;
    accentIndex: number; accentColor: string; autostartEnabled: boolean;
    history: Session[]; week: WeekDay[]; dbError: string | null; statusMessage: string | null;
  };

  let data: Snapshot | null = null;
  let currentScreen: 'home' | 'history' | 'settings' = 'home';
  let calendarWeek: WeekDay[] = [];
  let loadError = '';
  let saving: Record<string, boolean> = {};
  let isMaximized = false;
  let unlistenResize: (() => void) | undefined;

  const accents = [
    { name: 'Зелёный', color: '#52855b' },
    { name: 'Синий', color: '#2674df' },
    { name: 'Фиолетовый', color: '#8652c9' },
    { name: 'Розовый', color: '#e84b87' },
    { name: 'Оранжевый', color: '#f28a24' },
    { name: 'Красный', color: '#df3f3f' },
  ];

  async function refresh() {
    try {
      data = await invoke<Snapshot>('get_snapshot');
      loadError = '';
    } catch (error) {
      loadError = String(error);
    }
  }

  async function setPref(key: string, value: string | number | boolean) {
    if (!data) return;
    const encoded = String(value);
    const snapshotKey: Record<string, keyof Snapshot> = {
      goal_hours: 'goalHours', pause_minutes: 'pauseMinutes', layout_type: 'layoutType',
      theme: 'theme', accent_index: 'accentIndex', notify_goal: 'notifyGoal', autostart: 'autostartEnabled',
    };
    const target = snapshotKey[key];
    if (target) data = { ...data, [target]: typeof value === 'number' ? value : typeof value === 'boolean' ? value : encoded } as Snapshot;
    saving = { ...saving, [key]: true };
    try {
      await invoke('set_preference', { key, value: encoded });
      await refresh();
    } catch (error) {
      loadError = String(error);
      await refresh();
    } finally {
      saving = { ...saving, [key]: false };
    }
  }

  onMount(() => {
    void refresh();
    void appWindow.isMaximized().then((value) => isMaximized = value);
    void appWindow.onResized(() => {
      void appWindow.isMaximized().then((value) => isMaximized = value);
    }).then((unlisten) => unlistenResize = unlisten);
    const timer = window.setInterval(refresh, 1000);
    return () => {
      window.clearInterval(timer);
      unlistenResize?.();
    };
  });

  $: theme = data?.theme ?? 'light';
  $: accent = accents[data?.accentIndex ?? 0]?.color ?? '#52855b';
  $: progress = data ? Math.min(100, (data.todaySeconds / Math.max(1, data.goalSeconds)) * 100) : 0;
  $: ringProgress = Math.min(96, progress);
  $: filteredHistory = data?.history ?? [];
  $: calendarWeek = data ? mondayFirstWeek(data.week, data.today) : [];

  function duration(seconds = 0) {
    const h = Math.floor(seconds / 3600).toString().padStart(2, '0');
    const m = Math.floor((seconds % 3600) / 60).toString().padStart(2, '0');
    const s = Math.floor(seconds % 60).toString().padStart(2, '0');
    return `${h}:${m}:${s}`;
  }

  function hourWord(hours: number) {
    const lastTwo = hours % 100;
    if (lastTwo >= 11 && lastTwo <= 14) return 'часов';
    switch (hours % 10) {
      case 1: return 'час';
      case 2:
      case 3:
      case 4: return 'часа';
      default: return 'часов';
    }
  }

  function clock(value?: string | null) {
    if (!value) return '—';
    return value.slice(11, 16);
  }

  function dateLabel(value?: string) {
    const date = value ?? data?.today;
    if (!date) return '—';
    return new Intl.DateTimeFormat('ru-RU', { day: 'numeric', month: 'long', weekday: 'long' })
      .format(new Date(`${date}T12:00:00`));
  }

  function shortDay(value: string) {
    return new Intl.DateTimeFormat('ru-RU', { weekday: 'short' }).format(new Date(`${value}T12:00:00`)).replace('.', '');
  }

  function mondayFirstWeek(days: WeekDay[], today: string): WeekDay[] {
    const start = new Date(`${today}T12:00:00`);
    start.setDate(start.getDate() - ((start.getDay() + 6) % 7));
    return Array.from({ length: 7 }, (_, index) => {
      const date = new Date(start);
      date.setDate(start.getDate() + index);
      const key = [date.getFullYear(), `${date.getMonth() + 1}`.padStart(2, '0'), `${date.getDate()}`.padStart(2, '0')].join('-');
      return days.find((day) => day.date === key) ?? { date: key, label: '', seconds: 0 };
    });
  }

  function prettyDate(value: string) {
    return new Intl.DateTimeFormat('ru-RU', { day: 'numeric', month: 'short' }).format(new Date(`${value}T12:00:00`));
  }

  async function toggleWindowSize() {
    await appWindow.toggleMaximize();
    isMaximized = await appWindow.isMaximized();
  }

  function minimizeWindow() {
    void appWindow.minimize();
  }

  function closeToTray() {
    void appWindow.close();
  }

  function dragRegion(node: HTMLElement) {
    const onMouseDown = (event: MouseEvent) => {
      if (event.buttons !== 1) return;
      if (event.target instanceof Element && event.target.closest('.window-control')) return;
      if (event.detail === 2) {
        void toggleWindowSize();
      } else {
        void appWindow.startDragging();
      }
    };
    node.addEventListener('mousedown', onMouseDown);
    return { destroy: () => node.removeEventListener('mousedown', onMouseDown) };
  }
</script>

<svelte:head>
  <title>Ритм — учёт активности</title>
  <meta name="theme-color" content={theme === 'dark' ? '#121a17' : '#f5f7f2'} />
</svelte:head>

<main class:dark={theme === 'dark'} class="app-shell" style={`--accent:${accent}`}>
  <header class="topbar" use:dragRegion>
    <div class="brand-lockup">
      <span class="brand-mark"><span></span></span>
      <span class="brand-name">ритм</span>
      <span class="brand-caption">учёт активности</span>
    </div>
    <div class="titlebar-drag"></div>
    <div class="topbar-right">
      <div class="today-chip">
        <span class="today-dot"></span>
        <span>СЕГОДНЯ</span>
        <strong>{data?.today ? new Intl.DateTimeFormat('ru-RU', { day: '2-digit', month: '2-digit', year: 'numeric' }).format(new Date(`${data.today}T12:00:00`)) : '—'}</strong>
      </div>
      <div class="window-controls" aria-label="Управление окном">
        <button class="window-control" aria-label="Свернуть окно" title="Свернуть" onclick={minimizeWindow}>
          <svg aria-hidden="true" viewBox="0 0 16 16"><path d="M3 8h10" /></svg>
        </button>
        <button class="window-control" aria-label={isMaximized ? 'Восстановить окно' : 'Развернуть окно'} title={isMaximized ? 'Восстановить' : 'Развернуть'} onclick={toggleWindowSize}>
          {#if isMaximized}
            <svg aria-hidden="true" viewBox="0 0 16 16"><path d="M5 3.5h7.5V11M3.5 5H11v7.5H3.5z" /></svg>
          {:else}
            <svg aria-hidden="true" viewBox="0 0 16 16"><rect x="3.5" y="3.5" width="9" height="9" /></svg>
          {/if}
        </button>
        <button class="window-control close-control" aria-label="Закрыть и свернуть в трей" title="Свернуть в трей" onclick={closeToTray}>
          <svg aria-hidden="true" viewBox="0 0 16 16"><path d="m4 4 8 8M12 4l-8 8" /></svg>
        </button>
      </div>
    </div>
  </header>

  {#if loadError || data?.dbError}
    <div class="error-banner"><span class="error-icon">!</span>{loadError || data?.dbError}</div>
  {/if}

  <section class="screen-content" aria-label="Содержимое экрана">
    {#if !data}
      <div class="loading-card"><span class="loader"></span><span>Запускаем учёт активности…</span></div>
    {:else if currentScreen === 'home'}
      {#if data.layoutType === 'quiet'}
        <section class="quiet-screen">
          <h1 class="quiet-title">Главная</h1>
          <div class="quiet-stage">
            <div class="quiet-orbit" style={`--progress:${ringProgress};`}>
              <div class="quiet-timer-content">
                <span class="quiet-caption">Сегодня активность</span>
                <strong class="quiet-time">{duration(data.todaySeconds)}</strong>
                <span class="quiet-start">Запуск: {clock(data.startedAt)}</span>
                <div class:paused={!data.activeNow} class="quiet-status">
                  {#if data.activeNow}
                    <svg aria-hidden="true" viewBox="0 0 16 16"><path d="M5.5 4v8M10.5 4v8" /></svg>
                    <span>Активно</span>
                  {:else}
                    <svg aria-hidden="true" viewBox="0 0 16 16"><path d="m6 4.5 5 3.5-5 3.5z" /></svg>
                    <span>Пауза</span>
                  {/if}
                </div>
              </div>
            </div>
          </div>
        </section>
      {:else if data.layoutType === 'bento'}
        <section class="bento-layout">
          <div class="bento-intro"><h1>Главная</h1></div>
          <div class="bento-grid">
            <article class="bento-card bento-main">
              <span class="bento-main-label">Сегодня активность</span>
              <strong class="bento-time">{duration(data.todaySeconds)}</strong>
              <div class:paused={!data.activeNow} class="bento-status">
                {#if data.activeNow}
                  <svg aria-hidden="true" viewBox="0 0 16 16"><path d="M5.5 4v8M10.5 4v8" /></svg>
                  <span>Активно</span>
                {:else}
                  <svg aria-hidden="true" viewBox="0 0 16 16"><path d="m6 4.5 5 3.5-5 3.5z" /></svg>
                  <span>Пауза</span>
                {/if}
              </div>
            </article>
            <article class="bento-card bento-goal-tile">
              <div class="bento-goal-dial" style={`--progress:${progress};`}><strong>{Math.floor(progress)}%</strong></div>
              <span class="bento-goal-label">Цель: {data.goalHours} {hourWord(data.goalHours)}</span>
            </article>
            <article class="bento-card bento-start-tile">
              <svg class="bento-clock-icon" aria-hidden="true" viewBox="0 0 32 32"><circle cx="16" cy="16" r="12.5" /><path d="M16 8.5V16l5 3" /></svg>
              <div class="bento-start-copy"><span class="bento-start-label">Начало сессии</span><strong>{clock(data.startedAt)}</strong></div>
            </article>
          </div>
        </section>
      {:else}
        <section class="heatmap-home">
          <header class="heatmap-home-heading"><h1>Главная</h1></header>
          <div class="heatmap-home-grid">
            <article class="heat-focus">
              <span class="heat-focus-caption">Сегодня активность</span>
              <strong class="heat-focus-time">{duration(data.todaySeconds)}</strong>
              <span class="heat-focus-start">Запуск: {clock(data.startedAt)}</span>
              <div class:paused={!data.activeNow} class="heat-focus-status">
                {#if data.activeNow}
                  <svg aria-hidden="true" viewBox="0 0 16 16"><path d="M5.5 4v8M10.5 4v8" /></svg>
                  <span>Активно</span>
                {:else}
                  <svg aria-hidden="true" viewBox="0 0 16 16"><path d="m6 4.5 5 3.5-5 3.5z" /></svg>
                  <span>Пауза</span>
                {/if}
              </div>
            </article>
            <article class="heat-week-panel" aria-label="Активность по дням недели">
              <div class="heat-weekdays">
                {#each calendarWeek as day}
                  <span class:today={day.date === data.today}>{shortDay(day.date)}</span>
                {/each}
              </div>
              <div class="heat-week-grid">
                {#each [0, 1, 2, 3] as level}
                  <div class="heat-week-row">
                    {#each calendarWeek as day}
                      {@const dayProgress = Math.min(100, day.seconds / Math.max(1, data.goalSeconds) * 100)}
                      <span class="heat-week-cell" class:filled={dayProgress >= (level + 1) * 20} class:today={day.date === data.today} title={`${shortDay(day.date)}: ${duration(day.seconds)}`}></span>
                    {/each}
                  </div>
                {/each}
              </div>
              <div class="heat-goal-caption"><strong>Цель: {data.goalHours} {hourWord(data.goalHours)}</strong><span>{Math.floor(progress)}%</span></div>
              <div class="heat-goal-track" aria-label={`Выполнение цели: ${Math.floor(progress)}%`}><span style={`width:${progress}%`}></span></div>
            </article>
          </div>
        </section>
      {/if}
    {:else if currentScreen === 'history'}
      <section class="history-screen">
        <div class="history-heading"><h1>История</h1></div>
        <section class="history-card">
          <div class="table-head"><span>День</span><span>Запуск</span><span>Активность</span><span>Завершение</span></div>
          {#if filteredHistory.length}
            {#each filteredHistory as row}
              <div class="history-row" class:current={row.endedAt === null}>
                <div class="history-date"><strong>{prettyDate(row.day)}</strong></div>
                <span class="table-time">{clock(row.startedAt)}</span>
                <span class="table-duration">{duration(row.activeSeconds)}</span>
                <span class="table-end">{row.endedAt ? clock(row.endedAt) : 'В процессе'}</span>
              </div>
            {/each}
          {:else}
            <div class="empty-history"><span class="empty-clock">◷</span><strong>Пока нет записей</strong><span>Здесь появится дневная сводка активности.</span></div>
          {/if}
        </section>
      </section>
    {:else}
      <section class="settings-screen">
        <div class="settings-heading"><h1>Настройки</h1></div>
        <section class="settings-panel" aria-label="Настройки приложения">
          <div class="settings-row settings-goal-row">
            <span class="setting-name">Требуемое время активности</span>
            <div class="stepper settings-stepper"><button aria-label="Уменьшить цель" disabled={data.goalHours <= 1 || saving.goal_hours} onclick={() => setPref('goal_hours', data!.goalHours - 1)}>−</button><strong>{data.goalHours}<small> часов</small></strong><button aria-label="Увеличить цель" disabled={data.goalHours >= 24 || saving.goal_hours} onclick={() => setPref('goal_hours', data!.goalHours + 1)}>+</button></div>
            <button class="check-choice" class:checked={data.notifyGoal} role="checkbox" aria-checked={data.notifyGoal} onclick={() => setPref('notify_goal', !data!.notifyGoal)}><span class="check-box">{data.notifyGoal ? '✓' : ''}</span><span>Уведомить при достижении</span></button>
          </div>
          <div class="settings-row settings-pause-row">
            <span class="setting-name">Пауза после</span>
            <div class="stepper settings-stepper"><button aria-label="Уменьшить время паузы" disabled={data.pauseMinutes <= 0 || saving.pause_minutes} onclick={() => setPref('pause_minutes', data!.pauseMinutes - 1)}>−</button><strong>{data.pauseMinutes}<small> мин</small></strong><button aria-label="Увеличить время паузы" disabled={data.pauseMinutes >= 60 || saving.pause_minutes} onclick={() => setPref('pause_minutes', data!.pauseMinutes + 1)}>+</button></div>
            <span class="setting-hint">0–60 минут</span>
          </div>
          <div class="settings-row settings-choice-row">
            <span class="setting-name">Тип</span>
            <div class="layout-switch">
              <button class:selected={data.layoutType === 'quiet'} onclick={() => setPref('layout_type', 'quiet')}>Тихий таймер</button>
              <button class:selected={data.layoutType === 'bento'} onclick={() => setPref('layout_type', 'bento')}>Bento</button>
              <button class:selected={data.layoutType === 'heatmap'} onclick={() => setPref('layout_type', 'heatmap')}>Тепловая карта</button>
            </div>
          </div>
          <div class="settings-row settings-choice-row">
            <span class="setting-name">Тема</span>
            <div class="segmented"><button class:chosen={data.theme === 'light'} onclick={() => setPref('theme', 'light')}>Светлая</button><button class:chosen={data.theme === 'dark'} onclick={() => setPref('theme', 'dark')}>Тёмная</button></div>
          </div>
          <div class="settings-row settings-choice-row settings-color-row">
            <span class="setting-name">Расцветка</span>
            <div class="swatches">{#each accents as color, index}<button class:chosen={data.accentIndex === index} aria-label={color.name} title={color.name} style={`--swatch:${color.color}`} onclick={() => setPref('accent_index', index)}><span>{data.accentIndex === index ? '✓' : ''}</span></button>{/each}</div>
          </div>
          <div class="settings-row settings-autostart-row">
            <span class="setting-name">Запускать вместе с Windows</span>
            <button class="switch" class:on={data.autostartEnabled} role="switch" aria-label="Запускать вместе с Windows" aria-checked={data.autostartEnabled} onclick={() => setPref('autostart', !data!.autostartEnabled)}><span></span></button>
          </div>
        </section>
      </section>
    {/if}
  </section>

  <nav class="bottom-nav" aria-label="Основная навигация">
    <button class:active={currentScreen === 'home'} aria-current={currentScreen === 'home' ? 'page' : undefined} onclick={() => currentScreen = 'home'}><svg class="nav-icon nav-home" aria-hidden="true" viewBox="0 0 18 18"><path d="M2.4 8.1 9 2.8l6.6 5.3v7.1c0 .8-.6 1.4-1.4 1.4h-3.1v-5.1H6.9v5.1H3.8c-.8 0-1.4-.6-1.4-1.4z" /></svg><span>Главная</span></button>
    <button class:active={currentScreen === 'history'} aria-current={currentScreen === 'history' ? 'page' : undefined} onclick={() => currentScreen = 'history'}><svg class="nav-icon" aria-hidden="true" viewBox="0 0 24 24"><circle cx="12" cy="12" r="8.5" /><path d="M12 7v5l3.2 2" /></svg><span>История</span></button>
    <button class:active={currentScreen === 'settings'} aria-current={currentScreen === 'settings' ? 'page' : undefined} onclick={() => currentScreen = 'settings'}><svg class="nav-icon" aria-hidden="true" viewBox="0 0 24 24"><path d="M12 8.5a3.5 3.5 0 1 0 0 7 3.5 3.5 0 0 0 0-7Z" /><path d="m19.4 13.5 1.1.9-1.5 2.6-1.4-.6a7.6 7.6 0 0 1-1.7 1l-.2 1.5h-3l-.2-1.5a7.6 7.6 0 0 1-1.7-1l-1.4.6-1.5-2.6 1.1-.9a7.4 7.4 0 0 1 0-2l-1.1-.9 1.5-2.6 1.4.6a7.6 7.6 0 0 1 1.7-1l.2-1.5h3l.2 1.5a7.6 7.6 0 0 1 1.7 1l1.4-.6 1.5 2.6-1.1.9a7.4 7.4 0 0 1 0 2Z" /></svg><span>Настройки</span></button>
  </nav>
</main>
