import { and, asc, desc, eq, inArray } from "drizzle-orm";
import { getDb } from "@/db";
import { conversationMembers, conversations, messages, users } from "@/db/schema";
import { getSessionUser } from "@/lib/auth";
import { demoFounders, ensureDemoWorkspace } from "@/lib/demo";

async function isMember(conversationId: string, userId: string) {
  const [membership] = await getDb().select({ id: conversationMembers.id }).from(conversationMembers)
    .where(and(eq(conversationMembers.conversationId, conversationId), eq(conversationMembers.userId, userId))).limit(1);
  return Boolean(membership);
}

export async function GET(request: Request) {
  const user = await getSessionUser(request);
  if (!user) return Response.json({ error: "Sign in required" }, { status: 401 });
  await ensureDemoWorkspace();
  const db = getDb();
  const rows = await db.select({
    id: conversations.id, type: conversations.type, name: conversations.name, createdAt: conversations.createdAt,
  }).from(conversationMembers).innerJoin(conversations, eq(conversationMembers.conversationId, conversations.id))
    .where(eq(conversationMembers.userId, user.id)).orderBy(asc(conversations.createdAt));
  const ids = rows.map((row) => row.id);
  const memberRows = ids.length ? await db.select({
    conversationId: conversationMembers.conversationId, userId: users.id, fullName: users.fullName, email: users.email,
  }).from(conversationMembers).innerJoin(users, eq(conversationMembers.userId, users.id))
    .where(inArray(conversationMembers.conversationId, ids)) : [];
  const lastRows = ids.length ? await db.select({
    conversationId: messages.conversationId, body: messages.body, createdAt: messages.createdAt,
  }).from(messages).where(inArray(messages.conversationId, ids)).orderBy(desc(messages.createdAt)) : [];
  const conversationList = rows.map((row) => {
    const members = memberRows.filter((member) => member.conversationId === row.id);
    const other = members.find((member) => member.userId !== user.id);
    return { ...row, members, displayName: row.type === "dm" ? other?.fullName ?? "Direct message" : row.name, lastMessage: lastRows.find((message) => message.conversationId === row.id) ?? null };
  });
  const requested = new URL(request.url).searchParams.get("conversationId");
  const activeConversationId = requested && ids.includes(requested) ? requested : (ids[0] ?? "general");
  const messageRows = await db.select({
    id: messages.id, conversationId: messages.conversationId, body: messages.body, createdAt: messages.createdAt,
    userId: users.id, fullName: users.fullName, email: users.email,
  }).from(messages).innerJoin(users, eq(messages.userId, users.id))
    .where(eq(messages.conversationId, activeConversationId)).orderBy(asc(messages.createdAt)).limit(200);
  return Response.json({ founders: demoFounders, conversations: conversationList, messages: messageRows, activeConversationId });
}

export async function POST(request: Request) {
  const user = await getSessionUser(request);
  if (!user) return Response.json({ error: "Sign in required" }, { status: 401 });
  await ensureDemoWorkspace();
  const input = await request.json() as { action?: string; conversationId?: string; body?: string; name?: string; memberIds?: string[]; otherUserId?: string };
  const db = getDb();
  if (input.action === "send") {
    const body = input.body?.trim();
    if (!input.conversationId || !await isMember(input.conversationId, user.id)) return Response.json({ error: "Conversation unavailable." }, { status: 403 });
    if (!body || body.length > 1000) return Response.json({ error: "Message must be between 1 and 1,000 characters." }, { status: 400 });
    await db.insert(messages).values({ id: crypto.randomUUID(), conversationId: input.conversationId, userId: user.id, body, createdAt: new Date().toISOString() });
    return Response.json({ sent: true }, { status: 201 });
  }
  if (input.action === "create_group") {
    const name = input.name?.trim().replace(/^#/, "");
    if (!name || name.length > 40) return Response.json({ error: "Give the group a short name." }, { status: 400 });
    const id = crypto.randomUUID();
    const now = new Date().toISOString();
    const allowedIds = new Set(demoFounders.map((founder) => founder.id));
    const memberIds = Array.from(new Set([user.id, ...(input.memberIds ?? []).filter((memberId) => allowedIds.has(memberId as typeof demoFounders[number]["id"]))]));
    await db.insert(conversations).values({ id, type: "group", name, createdBy: user.id, createdAt: now });
    for (const userId of memberIds) await db.insert(conversationMembers).values({ id: crypto.randomUUID(), conversationId: id, userId, joinedAt: now });
    return Response.json({ conversationId: id }, { status: 201 });
  }
  if (input.action === "start_dm") {
    const other = demoFounders.find((founder) => founder.id === input.otherUserId && founder.id !== user.id);
    if (!other) return Response.json({ error: "Choose another founder." }, { status: 400 });
    const id = `dm:${[user.id, other.id].sort().join(":")}`;
    const now = new Date().toISOString();
    await db.insert(conversations).values({ id, type: "dm", name: null, createdBy: user.id, createdAt: now }).onConflictDoNothing();
    for (const userId of [user.id, other.id]) await db.insert(conversationMembers).values({ id: `${id}:${userId}`, conversationId: id, userId, joinedAt: now }).onConflictDoNothing();
    return Response.json({ conversationId: id }, { status: 201 });
  }
  return Response.json({ error: "Unknown chat action." }, { status: 400 });
}
