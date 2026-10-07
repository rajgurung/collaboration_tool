import { clearSessionCookie, deleteSession, getSessionUser } from "@/lib/auth";
import { demoFounders } from "@/lib/demo";

export async function GET(request: Request) {
  const user = await getSessionUser(request);
  if (!user) return Response.json({ user: null }, { status: 401 });
  if (!demoFounders.some((founder) => founder.id === user.id)) {
    await deleteSession(request);
    return Response.json({ user: null }, { status: 401, headers: { "set-cookie": clearSessionCookie() } });
  }
  return Response.json({ user: { id: user.id, fullName: user.fullName, email: user.email } });
}
