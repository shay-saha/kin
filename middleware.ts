import { NextResponse, type NextRequest } from "next/server";

export async function middleware(request: NextRequest) {
  if (request.nextUrl.pathname === "/graph" && request.nextUrl.searchParams.get("demo") === "1") return NextResponse.next();
  const backend = (process.env.KIN_BACKEND_URL || `http://127.0.0.1:${process.env.KIN_BRAIN_PORT || "8787"}`).replace(/\/$/, "");
  const endpoint = new URL(`${backend}/api/session`);
  endpoint.searchParams.set("path", request.nextUrl.pathname + request.nextUrl.search);
  let session: Response;
  try {
    session = await fetch(endpoint, {
      headers: { cookie: request.headers.get("cookie") || "", "x-forwarded-proto": request.nextUrl.protocol.replace(":", "") },
      cache: "no-store",
      signal: AbortSignal.timeout(15000),
    });
  } catch {
    return NextResponse.json({ error: "The backend is unavailable. Please try again." }, { status: 503 });
  }
  if (session.status === 401) {
    const target = new URL("/signin", request.url);
    target.searchParams.set("next", request.nextUrl.pathname + request.nextUrl.search);
    return NextResponse.redirect(target);
  }
  if (!session.ok) return NextResponse.json(await session.json(), { status: session.status });
  const data = await session.json();
  const response = data.destination
    ? NextResponse.redirect(new URL(data.destination, request.url))
    : NextResponse.next();
  for (const cookie of session.headers.getSetCookie()) response.headers.append("set-cookie", cookie);
  response.headers.set("Cache-Control", "private, no-store");
  return response;
}

export const config = {
  matcher: ["/family/:path*", "/stage/:path*", "/wearer/:path*", "/settings/:path*", "/onboarding/:path*", "/graph/:path*", "/remember/:path*", "/stories/:path*"],
};
