// MyGo IPMsg 最小闭环 demo 前端：联系人列表 + 单会话文本收发。
// 绑定来自 `mygo generate` 生成的 src/mygo.ts。
import { Chat, events, type Message, type User } from "./mygo";

const $ = <T extends HTMLElement>(selector: string) => document.querySelector<T>(selector)!;

let users: User[] = [];
let unreadMap: Record<string, number> = {};
let selected: string | null = null;
let sending = false;

function showError(message: string): void {
  const el = $("#error");
  el.textContent = message;
  el.hidden = false;
  window.setTimeout(() => {
    el.hidden = true;
  }, 4000);
}

function describeUser(u: User): string {
  const parts = [u.group || "未分组", u.addr, u.online ? "在线" : "离线"];
  return parts.join(" · ");
}

async function renderUsers(): Promise<void> {
  const [list, unread] = await Promise.all([Chat.listUsers(), Chat.unread()]);
  users = list;
  unreadMap = unread;
  const ul = $("#users");
  ul.textContent = "";
  if (users.length === 0) {
    const li = document.createElement("li");
    li.className = "empty";
    li.textContent = "尚未发现联系人，等待局域网回应…";
    ul.appendChild(li);
    return;
  }
  for (const u of users) {
    const li = document.createElement("li");
    li.dataset.key = u.key;
    if (u.key === selected) li.classList.add("selected");
    if (!u.online) li.classList.add("offline");

    const avatar = document.createElement("div");
    avatar.className = "avatar";
    avatar.textContent = (u.nick || "?").slice(0, 1);

    const who = document.createElement("div");
    who.className = "who";
    const name = document.createElement("div");
    name.className = "name";
    name.textContent = u.nick || u.key;
    const sub = document.createElement("div");
    sub.className = "sub";
    sub.textContent = describeUser(u);
    who.append(name, sub);

    li.append(avatar, who);
    const badge = unreadMap[u.key];
    if (badge && badge > 0) {
      const b = document.createElement("div");
      b.className = "badge";
      b.textContent = String(badge);
      li.appendChild(b);
    }
    li.addEventListener("click", () => void selectUser(u.key));
    ul.appendChild(li);
  }
}

function renderMessages(messages: Message[]): void {
  const box = $("#messages");
  box.textContent = "";
  const timeFmt = new Intl.DateTimeFormat("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
  for (const m of messages) {
    const row = document.createElement("div");
    row.className = m.out ? "row out" : "row";
    const bubble = document.createElement("div");
    bubble.className = "bubble";
    bubble.textContent = m.text;
    const meta = document.createElement("div");
    meta.className = "meta-line";
    const status = m.out ? (m.acked ? "已送达" : "已发送") : "";
    meta.textContent = [timeFmt.format(m.time), status].filter(Boolean).join(" · ");
    bubble.appendChild(meta);
    row.appendChild(bubble);
    box.appendChild(row);
  }
  box.scrollTop = box.scrollHeight;
}

async function loadHistory(): Promise<void> {
  if (!selected) return;
  const messages = await Chat.history(selected);
  renderMessages(messages);
}

async function selectUser(key: string): Promise<void> {
  selected = key;
  await Chat.markRead(key);
  const u = users.find((x) => x.key === key);
  $("#chatHeader").textContent = u ? `${u.nick}（${key}）` : key;
  ($("#input") as HTMLTextAreaElement).disabled = false;
  ($("#send") as HTMLButtonElement).disabled = false;
  await Promise.all([renderUsers(), loadHistory()]);
}

async function sendMessage(): Promise<void> {
  const input = $<HTMLTextAreaElement>("#input");
  const text = input.value.trim();
  if (!selected || !text || sending) return;
  sending = true;
  try {
    await Chat.send(selected, text);
    input.value = "";
    await loadHistory();
  } catch (err) {
    showError(`发送失败：${err instanceof Error ? err.message : String(err)}`);
  } finally {
    sending = false;
  }
}

function setComposer(enabled: boolean): void {
  const input = $<HTMLTextAreaElement>("#input");
  const send = $<HTMLButtonElement>("#send");
  input.disabled = !enabled;
  send.disabled = !enabled;
}

async function init(): Promise<void> {
  const info = await Chat.info();
  $("#meNick").textContent = info.nick;
  $("#meMeta").textContent = `端口 ${info.port} · ${info.hostname} · ${info.wireVersion}`;
  setComposer(false);
  await renderUsers();

  $("#refresh").addEventListener("click", () => {
    void Chat.refresh();
  });
  $("#composer").addEventListener("submit", (e) => {
    e.preventDefault();
    void sendMessage();
  });
  const input = $<HTMLTextAreaElement>("#input");
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void sendMessage();
    }
  });

  events.usersChanged.on(() => void renderUsers());
  events.messageChanged.on((peer) => {
    void (async () => {
      await renderUsers();
      if (peer === selected) {
        await loadHistory();
        await Chat.markRead(peer);
      }
    })();
  });
}

void init();
