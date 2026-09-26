/**
 * Who is calling, by network address, for the per-IP rate limits. The address itself is never
 * stored: limits are keyed by its SHA-256, or by HMAC-SHA256 keyed with RATE_LIMIT_PEPPER when
 * that is set (without a pepper, an IPv4 address can be found from its hash by trying them all).
 */
import { createHash, createHmac } from "node:crypto";
import { isIP } from "node:net";

/**
 * The client's address as the proxy in front of the site reports it (web/README.md, "Limits"):
 * the last hop of `X-Forwarded-For`, the one the nearest proxy appended, else `X-Real-IP`.
 * Earlier hops are whatever the client sent, so they are never read. The last hop comes first
 * because every common proxy writes it (Vercel overwrites the header; nginx and cloud load
 * balancers append to it), while some pass a client's own `X-Real-IP` straight through. A header
 * that holds no address falls through to the next one, so a malformed hop can't switch the
 * per-IP limits off. Null only when neither holds an address; then only the per-user limits apply.
 */
export function clientIp(headers: Headers): string | null {
  const lastHop = normalizeIp(
    headers.get("x-forwarded-for")?.split(",").at(-1),
  );
  if (lastHop !== null) return lastHop;
  return normalizeIp(headers.get("x-real-ip") ?? undefined);
}

/** The rate-limit key for the client's address, or null when there is no address. */
export function clientIpKey(
  headers: Headers,
  pepper: string | undefined,
): string | null {
  const ip = clientIp(headers);
  return ip === null ? null : ipKey(ip, pepper);
}

/**
 * 64 lowercase hex characters, the same for every address of one network (`limitNetwork`) in any
 * spelling.
 */
export function ipKey(ip: string, pepper: string | undefined): string {
  const network = limitNetwork(ip);
  return pepper
    ? createHmac("sha256", pepper).update(network).digest("hex")
    : createHash("sha256").update(network).digest("hex");
}

/**
 * What the per-IP limits count as one client: an IPv4 address, or the /64 an IPv6 address is in
 * (`2001:db8:1:2::/64`). Anyone given IPv6 gets at least a /64 and can send from any address in
 * it, so counting single IPv6 addresses would hand one client 2^64 separate limits.
 */
export function limitNetwork(ip: string): string {
  const hextets = ipv6Hextets(ip);
  if (hextets === null) return ip;
  return `${formatIpv6([...hextets.slice(0, 4), 0, 0, 0, 0])}/64`;
}

/**
 * One address in one spelling: without a port (`1.2.3.4:5678`, `[2001:db8::1]:443`) or zone
 * (`fe80::1%en0`), IPv6 in its canonical form (RFC 5952: lowercase, no leading zeros, the longest
 * run of zeros as `::`), and an IPv4 address mapped into IPv6 (`::ffff:1.2.3.4`) as plain IPv4.
 * Null when it isn't one.
 */
export function normalizeIp(value: string | undefined): string | null {
  let ip = value?.trim().toLowerCase() ?? "";
  const bracketed = /^\[([^\]]+)\](?::\d+)?$/.exec(ip);
  if (bracketed) ip = bracketed[1]!;
  else if (/^[\d.]+:\d+$/.test(ip)) ip = ip.slice(0, ip.lastIndexOf(":"));
  if (ip.includes(":")) ip = ip.replace(/%.*$/, "");
  if (isIP(ip) === 4) return ip;
  const hextets = ipv6Hextets(ip);
  if (hextets === null) return null;
  const mapped =
    hextets.slice(0, 5).every((h) => h === 0) && hextets[5] === 0xffff;
  return mapped
    ? [
        hextets[6]! >> 8,
        hextets[6]! & 0xff,
        hextets[7]! >> 8,
        hextets[7]! & 0xff,
      ].join(".")
    : formatIpv6(hextets);
}

/** The eight 16-bit groups of an IPv6 address (any valid spelling, no zone), else null. */
function ipv6Hextets(ip: string): number[] | null {
  if (isIP(ip) !== 6 || ip.includes("%")) return null;
  // A trailing dotted IPv4 part (`::ffff:1.2.3.4`) is the last two groups.
  const dotted = /^(.*:)(\d+\.\d+\.\d+\.\d+)$/.exec(ip);
  let text = ip;
  if (dotted) {
    const [a, b, c, d] = dotted[2]!.split(".").map(Number) as [
      number,
      number,
      number,
      number,
    ];
    text = `${dotted[1]}${((a << 8) | b).toString(16)}:${((c << 8) | d).toString(16)}`;
  }
  const [head, tail] = text.split("::") as [string, string | undefined];
  const groups = (part: string) =>
    part === "" ? [] : part.split(":").map((h) => parseInt(h, 16));
  const front = groups(head);
  if (tail === undefined) return front;
  const back = groups(tail);
  return [
    ...front,
    ...Array<number>(8 - front.length - back.length).fill(0),
    ...back,
  ];
}

/** RFC 5952 text for eight groups: the first longest run of two or more zeros becomes `::`. */
function formatIpv6(hextets: readonly number[]): string {
  let bestStart = -1;
  let bestLength = 1;
  for (let i = 0; i < 8;) {
    if (hextets[i] !== 0) {
      i += 1;
      continue;
    }
    let j = i;
    while (j < 8 && hextets[j] === 0) j += 1;
    if (j - i > bestLength) {
      bestStart = i;
      bestLength = j - i;
    }
    i = j;
  }
  const hex = (groups: readonly number[]) =>
    groups.map((h) => h.toString(16)).join(":");
  if (bestStart < 0) return hex(hextets);
  return `${hex(hextets.slice(0, bestStart))}::${hex(hextets.slice(bestStart + bestLength))}`;
}
