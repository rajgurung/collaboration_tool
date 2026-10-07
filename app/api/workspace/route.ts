import { asc, desc, eq } from "drizzle-orm";
import { getDb } from "@/db";
import { meetings, projects, taskNotes, tasks } from "@/db/schema";
import { getSessionUser } from "@/lib/auth";

const projectSeed = [
  { id: "product", name: "Product discovery", lane: "now", status: "Research", progress: 62, accent: "#ffb454", owner: "Alex", summary: "Validate the first release scope with customer research.", sortOrder: 1 },
  { id: "design", name: "Design system", lane: "now", status: "In progress", progress: 38, accent: "#df85ff", owner: "Maya", summary: "Create a consistent visual and interaction system.", sortOrder: 2 },
  { id: "compliance", name: "Operations setup", lane: "now", status: "Preparing", progress: 44, accent: "#72e5b4", owner: "Priya", summary: "Prepare the operating process and launch checklist.", sortOrder: 3 },
  { id: "pricing", name: "Commercial model", lane: "next", status: "Research", progress: 18, accent: "#6cb6ff", owner: "Sam", summary: "Confirm pricing, costs and first-year assumptions.", sortOrder: 4 },
  { id: "launch", name: "Launch experience", lane: "next", status: "Planned", progress: 12, accent: "#ff758f", owner: "Alex", summary: "Complete onboarding, launch content and final testing.", sortOrder: 5 },
  { id: "growth", name: "Growth experiments", lane: "later", status: "Idea", progress: 0, accent: "#c9d76a", owner: "All", summary: "Test retention, referrals and expansion opportunities.", sortOrder: 6 },
];
const taskSeed = [
  { id: "t1", title: "Complete customer interview synthesis", projectId: "product", status: "done", owner: "Alex", dueDate: "18 Sep", priority: "high", sortOrder: 1 },
  { id: "t2", title: "Create the first component library", projectId: "design", status: "progress", owner: "Maya", dueDate: "24 Sep", priority: "high", sortOrder: 2 },
  { id: "t3", title: "Document the launch workflow", projectId: "compliance", status: "progress", owner: "Priya", dueDate: "26 Sep", priority: "high", sortOrder: 3 },
  { id: "t4", title: "Build the pricing calculator", projectId: "pricing", status: "todo", owner: "Sam", dueDate: "30 Sep", priority: "medium", sortOrder: 4 },
  { id: "t5", title: "Confirm the release name", projectId: "product", status: "blocked", owner: "All", dueDate: "02 Oct", priority: "medium", sortOrder: 5 },
  { id: "t6", title: "Prepare the launch budget", projectId: "pricing", status: "todo", owner: "Alex", dueDate: "04 Oct", priority: "medium", sortOrder: 6 },
];
const meetingSeed = { id: "m1", title: "Team weekly", heldOn: "20 Sep 2026", summary: "Agreed to prioritise product discovery, design consistency and operational readiness before locking the first release.", attendees: "Alex, Maya, Priya, Sam", decisions: "Keep the first release focused; make ownership explicit; resolve blockers in the weekly review.", createdAt: "2026-09-20" };

async function seedIfNeeded() {
  const db = getDb();
  const existing = await db.select({ id: projects.id }).from(projects).limit(1);
  if (!existing.length) {
    await db.insert(projects).values(projectSeed);
    await db.insert(tasks).values(taskSeed);
    await db.insert(meetings).values(meetingSeed);
  }
}

export async function GET(request: Request) {
  try {
    const user = await getSessionUser(request);
    if (!user) return Response.json({ error: "Sign in required" }, { status: 401 });
    await seedIfNeeded();
    const db = getDb();
    const [projectRows, taskRows, meetingRows, noteRows] = await Promise.all([
      db.select().from(projects).orderBy(asc(projects.sortOrder)),
      db.select().from(tasks).orderBy(asc(tasks.sortOrder)),
      db.select().from(meetings).orderBy(desc(meetings.createdAt)),
      db.select().from(taskNotes).orderBy(asc(taskNotes.createdAt)),
    ]);
    return Response.json({ projects: projectRows, tasks: taskRows, meetings: meetingRows, notes: noteRows });
  } catch (error) {
    return Response.json({ error: error instanceof Error ? error.message : "Workspace unavailable" }, { status: 500 });
  }
}

export async function POST(request: Request) {
  try {
    const user = await getSessionUser(request);
    if (!user) return Response.json({ error: "Sign in required" }, { status: 401 });
    const input = await request.json() as Record<string, string>;
    const db = getDb();
    if (input.action === "create_task") {
      if (!input.title?.trim() || !input.owner || !input.projectId) return Response.json({ error: "Missing task details" }, { status: 400 });
      const task = { id: crypto.randomUUID(), title: input.title.trim(), projectId: input.projectId, status: "todo", owner: input.owner, dueDate: "Not set", priority: input.priority || "medium", sortOrder: Date.now() };
      await db.insert(tasks).values(task);
      return Response.json({ task }, { status: 201 });
    }
    if (input.action === "update_task_status") {
      if (!input.id || !["todo", "progress", "blocked", "done"].includes(input.status)) return Response.json({ error: "Invalid task update" }, { status: 400 });
      await db.update(tasks).set({ status: input.status }).where(eq(tasks.id, input.id));
      return Response.json({ updated: true });
    }
    if (input.action === "create_task_note") {
      if (!input.taskId || !input.body?.trim()) return Response.json({ error: "A note message is required" }, { status: 400 });
      const note = { id: crypto.randomUUID(), taskId: input.taskId, author: user.fullName, body: input.body.trim(), createdAt: new Date().toISOString() };
      await db.insert(taskNotes).values(note);
      return Response.json({ note }, { status: 201 });
    }
    if (input.action === "create_meeting") {
      if (!input.title?.trim() || !input.summary?.trim()) return Response.json({ error: "Meeting title and minutes are required" }, { status: 400 });
      const now = new Date();
      const meeting = { id: crypto.randomUUID(), title: input.title.trim(), heldOn: new Intl.DateTimeFormat("en-GB", { day: "2-digit", month: "short", year: "numeric" }).format(now), summary: input.summary.trim(), attendees: "Alex, Maya, Priya, Sam", decisions: input.decisions?.trim() || "", createdAt: now.toISOString() };
      await db.insert(meetings).values(meeting);
      return Response.json({ meeting }, { status: 201 });
    }
    return Response.json({ error: "Unknown action" }, { status: 400 });
  } catch (error) {
    return Response.json({ error: error instanceof Error ? error.message : "Could not save changes" }, { status: 500 });
  }
}
