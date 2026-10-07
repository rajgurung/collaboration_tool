import { getDb } from "@/db";
import { conversationMembers, conversations, users } from "@/db/schema";

export const demoFounders = [
  { id: "founder-alex", fullName: "Alex", email: "alex@example.com" },
  { id: "founder-maya", fullName: "Maya", email: "maya@example.com" },
  { id: "founder-priya", fullName: "Priya", email: "priya@example.com" },
  { id: "founder-sam", fullName: "Sam", email: "sam@example.com" },
] as const;

export async function ensureDemoWorkspace() {
  const db = getDb();
  const now = new Date().toISOString();
  for (const founder of demoFounders) {
    await db.insert(users).values({ ...founder, passwordHash: "demo-only", passwordSalt: "demo-only", createdAt: now }).onConflictDoNothing();
  }
  await db.insert(conversations).values({ id: "general", type: "group", name: "general", createdBy: "founder-alex", createdAt: now }).onConflictDoNothing();
  for (const founder of demoFounders) {
    await db.insert(conversationMembers).values({ id: `general:${founder.id}`, conversationId: "general", userId: founder.id, joinedAt: now }).onConflictDoNothing();
  }
}
