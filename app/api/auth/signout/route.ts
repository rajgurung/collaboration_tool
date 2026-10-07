import { clearSessionCookie, deleteSession } from "@/lib/auth";

export async function POST(request: Request) {
  await deleteSession(request);
  return Response.json({ signedOut: true }, { headers: { "set-cookie": clearSessionCookie() } });
}
