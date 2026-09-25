-- Per-user quotas on usage readings: when each reading was stored (the "new readings in 24
-- hours" quota counts these) and every window id a user has stored on an account (the cap on
-- distinct window ids counts these, and they outlive the readings' 90-day retention).

-- AlterTable
ALTER TABLE "UsageReading" ADD COLUMN     "createdAt" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP;

-- CreateTable
CREATE TABLE "UsageWindow" (
    "userId" TEXT NOT NULL,
    "accountKey" TEXT NOT NULL,
    "windowId" TEXT NOT NULL,
    "firstSeenAt" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "UsageWindow_pkey" PRIMARY KEY ("userId","accountKey","windowId")
);

-- CreateIndex
CREATE INDEX "UsageReading_userId_createdAt_idx" ON "UsageReading"("userId", "createdAt");

-- AddForeignKey
ALTER TABLE "UsageWindow" ADD CONSTRAINT "UsageWindow_userId_fkey" FOREIGN KEY ("userId") REFERENCES "User"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "UsageWindow" ADD CONSTRAINT "UsageWindow_accountKey_fkey" FOREIGN KEY ("accountKey") REFERENCES "ClaudeAccount"("key") ON DELETE CASCADE ON UPDATE CASCADE;

-- Windows already stored count towards the cap (added by hand).
INSERT INTO "UsageWindow" ("userId", "accountKey", "windowId", "firstSeenAt")
SELECT "userId", "accountKey", "windowId", MIN("observedAt")
FROM "UsageReading"
GROUP BY "userId", "accountKey", "windowId";

-- Row-level security, as for every table (see 0001_init).
ALTER TABLE "UsageWindow" ENABLE ROW LEVEL SECURITY;
