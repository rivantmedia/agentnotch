/**
 * How people are shown to each other in a pool: by their Google-verified email, with the name
 * (which anyone can rewrite in their user_metadata) only beside it.
 */
import { describe, expect, it } from "vitest";

import { displayName, memberLabel } from "~/server/services/users";

describe("memberLabel", () => {
  it("leads with the email and keeps the name secondary", () => {
    expect(memberLabel({ email: "ann@example.com", name: "Ann" })).toEqual({
      displayName: "ann@example.com",
      name: "Ann",
    });
  });

  it("never lets a chosen name stand in for the email", () => {
    // Bob renames himself "Ann": he is still shown as bob@….
    expect(
      memberLabel({ email: "bob@example.com", name: "Ann" }).displayName,
    ).toBe("bob@example.com");
    expect(memberLabel({ email: "", name: "Ann" })).toEqual({
      displayName: "Member",
      name: "Ann",
    });
  });

  it("drops empty names and names that just repeat the email", () => {
    expect(memberLabel({ email: "a@x.io", name: "  " }).name).toBeNull();
    expect(memberLabel({ email: "a@x.io", name: null }).name).toBeNull();
    expect(memberLabel({ email: "a@x.io", name: "a@x.io" }).name).toBeNull();
  });
});

describe("displayName (the viewer's own greeting)", () => {
  it("prefers the name, then the email's local part", () => {
    expect(displayName({ email: "ann@example.com", name: "Ann" })).toBe("Ann");
    expect(displayName({ email: "ann@example.com", name: null })).toBe("ann");
  });
});
