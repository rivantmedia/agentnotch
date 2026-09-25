/**
 * Keeps the `User` row in step with the identity a verified token carries.
 */
import { type TokenIdentity } from "~/server/auth/verify-bearer";
import { type Db } from "~/server/db-types";

export type ViewerUser = { id: string; email: string; name: string | null };

/**
 * Creates the user's row on first sight and refreshes email and name when the token carries new
 * ones. A token without an email or name never blanks what is stored.
 */
export async function ensureUser(
  db: Db,
  identity: TokenIdentity,
): Promise<ViewerUser> {
  const select = { id: true, email: true, name: true } as const;
  const existing = await db.user.findUnique({
    where: { id: identity.id },
    select,
  });
  if (existing) {
    const email = identity.email ?? existing.email;
    const name = identity.name ?? existing.name;
    if (email === existing.email && name === existing.name) return existing;
    return db.user.update({
      where: { id: identity.id },
      data: { email, name },
      select,
    });
  }
  // Two first requests can race; the upsert makes the loser a no-op update.
  return db.user.upsert({
    where: { id: identity.id },
    create: {
      id: identity.id,
      email: identity.email ?? "",
      name: identity.name,
    },
    update: {},
    select,
  });
}

/**
 * How the site greets the signed-in person themself: their name, else their email's local part.
 * Never used to show one person to another (see `memberLabel`).
 */
export function displayName(user: {
  name: string | null;
  email: string;
}): string {
  const name = user.name?.trim();
  if (name) return name;
  const local = user.email.split("@")[0]?.trim();
  return local && local.length > 0 ? local : "Member";
}

/**
 * How a person is shown to the other members of a pool: by their email, which Google verified
 * and they can't make look like someone else's, with their name as a secondary label. The name
 * comes from `user_metadata`, which anyone can rewrite, so it is never shown on its own.
 */
export function memberLabel(user: { name: string | null; email: string }): {
  displayName: string;
  name: string | null;
} {
  const email = user.email.trim();
  const name = user.name?.trim() ?? "";
  return {
    displayName: email.length > 0 ? email : "Member",
    name: name.length > 0 && name !== email ? name : null,
  };
}
