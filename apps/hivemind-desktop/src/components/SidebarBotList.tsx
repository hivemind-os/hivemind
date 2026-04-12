import { For, Show } from 'solid-js';
import { LayoutGrid, Lock, HelpCircle } from 'lucide-solid';
import {
  SidebarGroup,
  SidebarGroupContent,
  SidebarMenu,
  SidebarMenuItem,
  SidebarMenuButton,
  SidebarMenuBadge,
} from '~/ui';
import type { BotStore } from '../stores/botStore';

interface SidebarBotListProps {
  store: BotStore;
  onSelectBot?: (bot_id: string) => void;
}

const ALL_STATUSES: Array<{ value: string; label: string }> = [
  { value: 'all', label: 'All statuses' },
  { value: 'active', label: 'Active' },
  { value: 'spawning', label: 'Spawning' },
  { value: 'waiting', label: 'Waiting' },
  { value: 'paused', label: 'Paused' },
  { value: 'blocked', label: 'Blocked' },
  { value: 'done', label: 'Done' },
  { value: 'error', label: 'Error' },
];

function SidebarBotList(props: SidebarBotListProps) {
  return (
    <SidebarGroup>
      <SidebarGroupContent>
        {/* Search + Filter row */}
        <div class="flex flex-col gap-1.5 px-2 py-2">
          <input
            type="text"
            placeholder="Search bots…"
            value={props.store.searchQuery()}
            onInput={(e) => props.store.setSearchQuery(e.currentTarget.value)}
            class="bg-sidebar-accent text-sidebar-foreground border border-sidebar-border rounded-md px-2 py-1 text-xs w-full"
          />
          <select
            value={props.store.statusFilter()}
            onChange={(e) => props.store.setStatusFilter(e.currentTarget.value as any)}
            class="bg-sidebar-accent text-sidebar-foreground border border-sidebar-border rounded-md px-2 py-1 text-xs w-full"
          >
            <For each={ALL_STATUSES}>
              {(s) => <option value={s.value}>{s.label}</option>}
            </For>
          </select>
        </div>

        <SidebarMenu>
          {/* Stage item — always first */}
          <SidebarMenuItem>
            <SidebarMenuButton
              isActive={props.store.selectedBotId() === null}
              onClick={() => props.store.selectBot(null)}
              size="sm"
            >
              <LayoutGrid size={14} />
              <span class="truncate">Stage</span>
            </SidebarMenuButton>
          </SidebarMenuItem>

          {/* Bot list */}
          <For each={props.store.filteredBots()}>
            {(bot) => {
              const approvals = () => props.store.approvalsForBot(bot.config.id);
              const questions = () => props.store.questionsForBot(bot.config.id);

              return (
                <SidebarMenuItem>
                  <SidebarMenuButton
                    isActive={bot.config.id === props.store.selectedBotId()}
                    onClick={() => {
                      if (props.onSelectBot) {
                        props.onSelectBot(bot.config.id);
                      } else {
                        props.store.selectBot(bot.config.id);
                      }
                    }}
                    class="gap-1.5"
                    size="sm"
                  >
                    <span
                      class="inline-block size-2 shrink-0 rounded-full"
                      classList={{
                        'bg-green-400': bot.status === 'active',
                        'bg-yellow-400':
                          bot.status === 'spawning' ||
                          bot.status === 'waiting' ||
                          bot.status === 'paused',
                        'bg-red-400':
                          bot.status === 'error' || bot.status === 'blocked',
                        'bg-slate-600': bot.status === 'done',
                      }}
                    />
                    <Show when={bot.config.avatar}>
                      <span class="shrink-0">{bot.config.avatar}</span>
                    </Show>
                    <span class="flex flex-col min-w-0">
                      <span class="truncate">{bot.config.friendly_name}</span>
                      <Show when={bot.config.persona_id}>
                        <span class="truncate text-[10px] text-muted-foreground leading-tight">{bot.config.persona_id}</span>
                      </Show>
                    </span>
                  </SidebarMenuButton>

                  <Show when={approvals() > 0 || questions() > 0}>
                    <SidebarMenuBadge>
                      <Show when={approvals() > 0}>
                        <span
                          class="flex items-center gap-0.5 text-amber-400"
                          title="Pending approvals"
                        >
                          <Lock size={12} />
                          <span class="text-[10px]">{approvals()}</span>
                        </span>
                      </Show>
                      <Show when={questions() > 0}>
                        <span
                          class="flex items-center gap-0.5 text-blue-400"
                          title="Pending questions"
                        >
                          <HelpCircle size={12} />
                          <span class="text-[10px]">{questions()}</span>
                        </span>
                      </Show>
                    </SidebarMenuBadge>
                  </Show>
                </SidebarMenuItem>
              );
            }}
          </For>
        </SidebarMenu>

        {/* Empty state */}
        <Show when={props.store.filteredBots().length === 0}>
          <p class="px-2 text-xs text-muted-foreground">No bots running.</p>
        </Show>
      </SidebarGroupContent>
    </SidebarGroup>
  );
}

export default SidebarBotList;
