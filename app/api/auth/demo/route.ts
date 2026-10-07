import { createSession, sessionCookie } from "@/lib/auth";
import { demoFounders, ensureDemoWorkspace } from "@/lib/demo";

export async function POST(request: Request) {
  const input = await request.json() as { founderId?: string };
  const founder = demoFounders.find((item) => item.id === input.founderId);
  if (!founder) return Response.json({ error: "Choose a founder to continue." }, { status: 400 });
  await ensureDemoWorkspace();
  const session = await createSession(founder.id);
  return Response.json({ user: founder }, { headers: { "set-cookie": sessionCookie(session.token, session.expiresAt) } });
}
