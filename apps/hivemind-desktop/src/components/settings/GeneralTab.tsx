import { Show, For, createSignal, createEffect } from 'solid-js';
import type { Accessor, JSX } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { authFetch } from '~/lib/authFetch';
import { Switch, SwitchControl, SwitchThumb, SwitchLabel } from '~/ui/switch';
import type { AppContext, HiveMindConfigData } from '../../types';

export interface GeneralTabProps {
  editConfig: Accessor<HiveMindConfigData | null>;
  updateAfk: (field: string, value: unknown) => void;
  context: Accessor<AppContext | null>;
  daemonOnline: Accessor<boolean>;
  active: Accessor<boolean>;
  configLoadingFallback: (label: string) => JSX.Element;
}

const GeneralTab = (props: GeneralTabProps) => {
  const editConfig = props.editConfig;
  const updateAfk = props.updateAfk;

  // Connectors list for AFK forwarding dropdown
  const [afkConnectors, setAfkConnectors] = createSignal<Array<{ id: string; name: string; provider: string }>>([]);
  // Channels for the selected chat connector (Slack/Discord)
  const [afkChannels, setAfkChannels] = createSignal<Array<{ id: string; name: string; group_name?: string; channel_type?: string }>>([]);
  const [afkChannelsLoading, setAfkChannelsLoading] = createSignal(false);

  const loadAfkConnectors = async () => {
    const url = props.context()?.daemon_url;
    if (!url) return;
    try {
      const resp = await authFetch(`${url}/api/v1/config/connectors`);
      if (resp.ok) {
        const data = await resp.json();
        if (Array.isArray(data)) {
          setAfkConnectors(data.map((c: any) => ({ id: c.id, name: c.name || c.id, provider: c.provider || '' })));
        }
      }
    } catch (_) { /* daemon may be offline */ }
  };

  // Load connectors when AFK tab is shown
  createEffect(() => {
    if (props.active() && props.daemonOnline()) {
      void loadAfkConnectors();
    }
  });

  // Derive whether the selected AFK connector is chat-based (Slack/Discord) or email
  const selectedAfkConnector = () => {
    const cfg = editConfig();
    const cid = cfg?.afk?.forward_channel_id;
    return cid ? afkConnectors().find((c) => c.id === cid) : undefined;
  };
  const afkConnectorIsChat = () => {
    const p = selectedAfkConnector()?.provider?.toLowerCase() ?? '';
    return p === 'discord' || p === 'slack';
  };

  const afk = () => editConfig()?.afk ?? {} as NonNullable<HiveMindConfigData['afk']>;

  // Fetch channels when a chat connector is selected
  createEffect(() => {
    const conn = selectedAfkConnector();
    if (conn && afkConnectorIsChat()) {
      setAfkChannelsLoading(true);
      invoke<any[]>('list_connector_channels', { connector_id: conn.id })
        .then((chs) => setAfkChannels((chs ?? []).map((ch: any) => ({
          id: ch.id ?? '',
          name: ch.name ?? ch.id ?? '',
          group_name: ch.group_name,
          channel_type: ch.channel_type,
        }))))
        .catch(() => setAfkChannels([]))
        .finally(() => setAfkChannelsLoading(false));
    } else {
      setAfkChannels([]);
    }
  });

  return (
    <div class="settings-section">
      <Show when={editConfig()} fallback={props.configLoadingFallback('AFK settings')}>
            <>
              <h3>Auto-Status Transitions</h3>
              <p class="muted">Automatically change your status based on inactivity. The heartbeat is sent by the desktop UI when you interact with it.</p>
              <div class="settings-form">
                <label>
                  <span>Auto-idle after (seconds)</span>
                  <input
                    type="number" min="30" max="7200" step="30"
                    value={afk().auto_idle_after_secs ?? 300}
                    onChange={(e) => updateAfk('auto_idle_after_secs', parseInt(e.currentTarget.value) || null)}
                  />
                  <span class="muted" style="font-size:0.8em;">Set to blank to disable auto-idle</span>
                </label>
                <label>
                  <span>Auto-away after (seconds)</span>
                  <input
                    type="number" min="60" max="14400" step="60"
                    value={afk().auto_away_after_secs ?? 900}
                    onChange={(e) => updateAfk('auto_away_after_secs', parseInt(e.currentTarget.value) || null)}
                  />
                  <span class="muted" style="font-size:0.8em;">Set to blank to disable auto-away</span>
                </label>
              </div>

              <h3>Interaction Forwarding</h3>
              <p class="muted">When your status is Away or Do Not Disturb, pending approvals and questions can be forwarded to a communication channel.</p>
              <div class="settings-form">
                <label>
                  <span>Forward channel</span>
                  {(() => {
                    let selectRef!: HTMLSelectElement;
                    createEffect(() => {
                      const val = afk().forward_channel_id ?? '';
                      const _connectors = afkConnectors();
                      if (selectRef) {
                        queueMicrotask(() => { selectRef.value = val; });
                      }
                    });
                    return (
                      <select
                        ref={selectRef}
                        onInput={(e) => updateAfk('forward_channel_id', e.currentTarget.value || null)}
                      >
                        <option value="">None</option>
                        <For each={afkConnectors()}>
                          {(c) => (
                            <option value={c.id}>
                              {c.name}{c.provider ? ` (${c.provider})` : ''}
                            </option>
                          )}
                        </For>
                      </select>
                    );
                  })()}
                  <span class="muted" style="font-size:0.8em;">The connector to send interaction notifications to</span>
                </label>
                <Show when={afkConnectorIsChat()} fallback={
                  <label>
                    <span>Recipient address</span>
                    <input
                      type="text"
                      placeholder="you@example.com"
                      value={afk().forward_to_address ?? ''}
                      onChange={(e) => updateAfk('forward_to_address', e.currentTarget.value || null)}
                    />
                    <span class="muted" style="font-size:0.8em;">Required for email connectors</span>
                  </label>
                }>
                  <label>
                    <span>Send to channel</span>
                    {(() => {
                      let chanRef!: HTMLSelectElement;
                      createEffect(() => {
                        const val = afk().forward_to_address ?? '';
                        const _channels = afkChannels();
                        if (chanRef) {
                          queueMicrotask(() => { chanRef.value = val; });
                        }
                      });
                      return (
                        <select
                          ref={chanRef}
                          disabled={afkChannelsLoading()}
                          onInput={(e) => updateAfk('forward_to_address', e.currentTarget.value || null)}
                        >
                          <option value="">{afkChannelsLoading() ? 'Loading channels…' : '— Default channel —'}</option>
                          <For each={afkChannels()}>
                            {(ch) => (
                              <option value={ch.id}>
                                {ch.group_name ? `${ch.group_name} / ` : ''}{ch.name}{ch.channel_type ? ` (${ch.channel_type})` : ''}
                              </option>
                            )}
                          </For>
                        </select>
                      );
                    })()}
                    <span class="muted" style="font-size:0.8em;">The channel to send notifications to (leave blank for the connector's default)</span>
                  </label>
                </Show>
                <Switch checked={afk().forward_approvals ?? true} onChange={(checked) => updateAfk('forward_approvals', checked)} class="flex items-center gap-2">

                  <SwitchControl><SwitchThumb /></SwitchControl>

                  <SwitchLabel>Forward tool approvals</SwitchLabel>

                </Switch>
                <Switch checked={afk().forward_questions ?? true} onChange={(checked) => updateAfk('forward_questions', checked)} class="flex items-center gap-2">

                  <SwitchControl><SwitchThumb /></SwitchControl>

                  <SwitchLabel>Forward agent questions</SwitchLabel>

                </Switch>
                <label>
                  <span>Auto-approve timeout (seconds)</span>
                  <input
                    type="number" min="30" max="3600" step="30"
                    value={afk().auto_approve_on_timeout_secs ?? ''}
                    placeholder="Disabled"
                    onChange={(e) => updateAfk('auto_approve_on_timeout_secs', parseInt(e.currentTarget.value) || null)}
                  />
                  <span class="muted" style="font-size:0.8em;">Automatically approve tool requests after this many seconds (leave blank to disable)</span>
                </label>
              </div>
            </>
      </Show>
    </div>
  );
};

export default GeneralTab;
