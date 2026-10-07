import { index, integer, sqliteTable, text, uniqueIndex } from "drizzle-orm/sqlite-core";
export const projects = sqliteTable("projects", { id: text("id").primaryKey(), name: text("name").notNull(), lane: text("lane").notNull(), status: text("status").notNull(), progress: integer("progress").notNull().default(0), accent: text("accent").notNull(), owner: text("owner").notNull(), summary: text("summary").notNull(), sortOrder: integer("sort_order").notNull().default(0) });
export const tasks = sqliteTable("tasks", { id: text("id").primaryKey(), title: text("title").notNull(), projectId: text("project_id").notNull().references(() => projects.id), status: text("status").notNull().default("todo"), owner: text("owner").notNull(), dueDate: text("due_date").notNull(), priority: text("priority").notNull().default("medium"), sortOrder: integer("sort_order").notNull().default(0) });
export const meetings = sqliteTable("meetings", { id: text("id").primaryKey(), title: text("title").notNull(), heldOn: text("held_on").notNull(), summary: text("summary").notNull(), attendees: text("attendees").notNull(), decisions: text("decisions").notNull().default(""), createdAt: text("created_at").notNull() });
export const taskNotes = sqliteTable("task_notes", { id: text("id").primaryKey(), taskId: text("task_id").notNull().references(() => tasks.id), author: text("author").notNull(), body: text("body").notNull(), createdAt: text("created_at").notNull() });
export const users = sqliteTable("users", {
  id: text("id").primaryKey(),
  fullName: text("full_name").notNull(),
  email: text("email").notNull(),
  passwordHash: text("password_hash").notNull(),
  passwordSalt: text("password_salt").notNull(),
  createdAt: text("created_at").notNull(),
}, (table) => [uniqueIndex("users_email_unique").on(table.email)]);
export const sessions = sqliteTable("sessions", {
  id: text("id").primaryKey(),
  userId: text("user_id").notNull().references(() => users.id, { onDelete: "cascade" }),
  expiresAt: integer("expires_at").notNull(),
  createdAt: text("created_at").notNull(),
}, (table) => [index("sessions_user_id_idx").on(table.userId), index("sessions_expires_at_idx").on(table.expiresAt)]);
export const chatMessages = sqliteTable("chat_messages", {
  id: text("id").primaryKey(),
  userId: text("user_id").notNull().references(() => users.id, { onDelete: "cascade" }),
  body: text("body").notNull(),
  createdAt: text("created_at").notNull(),
}, (table) => [index("chat_messages_created_at_idx").on(table.createdAt)]);
export const conversations = sqliteTable("conversations", {
  id: text("id").primaryKey(),
  type: text("type").notNull(),
  name: text("name"),
  createdBy: text("created_by").notNull().references(() => users.id),
  createdAt: text("created_at").notNull(),
});
export const conversationMembers = sqliteTable("conversation_members", {
  id: text("id").primaryKey(),
  conversationId: text("conversation_id").notNull().references(() => conversations.id, { onDelete: "cascade" }),
  userId: text("user_id").notNull().references(() => users.id, { onDelete: "cascade" }),
  joinedAt: text("joined_at").notNull(),
}, (table) => [
  uniqueIndex("conversation_members_unique").on(table.conversationId, table.userId),
  index("conversation_members_user_id_idx").on(table.userId),
]);
export const messages = sqliteTable("messages", {
  id: text("id").primaryKey(),
  conversationId: text("conversation_id").notNull().references(() => conversations.id, { onDelete: "cascade" }),
  userId: text("user_id").notNull().references(() => users.id, { onDelete: "cascade" }),
  body: text("body").notNull(),
  createdAt: text("created_at").notNull(),
}, (table) => [index("messages_conversation_created_at_idx").on(table.conversationId, table.createdAt)]);
