-- CreateSchema
CREATE SCHEMA IF NOT EXISTS "public";

-- CreateTable
CREATE TABLE "User" (
    "id" TEXT NOT NULL,
    "email" TEXT NOT NULL,
    "name" TEXT,
    "createdAt" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "User_pkey" PRIMARY KEY ("id")
);

-- CreateTable
CREATE TABLE "Device" (
    "userId" TEXT NOT NULL,
    "id" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "appVersion" TEXT NOT NULL,
    "lastSeenAt" TIMESTAMP(3) NOT NULL,

    CONSTRAINT "Device_pkey" PRIMARY KEY ("userId","id")
);

-- CreateTable
CREATE TABLE "ClaudeAccount" (
    "key" TEXT NOT NULL,
    "createdAt" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "ClaudeAccount_pkey" PRIMARY KEY ("key")
);

-- CreateTable
CREATE TABLE "UserAccount" (
    "userId" TEXT NOT NULL,
    "accountKey" TEXT NOT NULL,
    "email" TEXT,
    "organizationName" TEXT,
    "plan" TEXT,
    "label" TEXT,
    "firstSeenAt" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "lastSeenAt" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "UserAccount_pkey" PRIMARY KEY ("userId","accountKey")
);

-- CreateTable
CREATE TABLE "Project" (
    "id" TEXT NOT NULL,
    "userId" TEXT NOT NULL,
    "accountKey" TEXT NOT NULL,
    "key" TEXT NOT NULL,
    "name" TEXT NOT NULL,

    CONSTRAINT "Project_pkey" PRIMARY KEY ("id")
);

-- CreateTable
CREATE TABLE "Session" (
    "id" TEXT NOT NULL,
    "userId" TEXT NOT NULL,
    "accountKey" TEXT NOT NULL,
    "projectId" TEXT NOT NULL,
    "sessionId" TEXT NOT NULL,
    "title" TEXT,
    "source" TEXT NOT NULL,
    "models" TEXT[],
    "startedAt" TIMESTAMP(3) NOT NULL,
    "lastActivityAt" TIMESTAMP(3) NOT NULL,
    "endedAt" TIMESTAMP(3),
    "messageCount" INTEGER NOT NULL,
    "inputTokens" BIGINT NOT NULL,
    "outputTokens" BIGINT NOT NULL,
    "cacheCreationTokens" BIGINT NOT NULL,
    "cacheReadTokens" BIGINT NOT NULL,
    "costUsd" DECIMAL(14,6),
    "summaryText" TEXT,
    "summaryModel" TEXT,
    "summaryAt" TIMESTAMP(3),
    "deviceId" TEXT,
    "createdAt" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" TIMESTAMP(3) NOT NULL,

    CONSTRAINT "Session_pkey" PRIMARY KEY ("id")
);

-- CreateTable
CREATE TABLE "UsageReading" (
    "id" BIGSERIAL NOT NULL,
    "userId" TEXT NOT NULL,
    "accountKey" TEXT NOT NULL,
    "source" TEXT NOT NULL,
    "windowId" TEXT NOT NULL,
    "utilization" DOUBLE PRECISION NOT NULL,
    "resetsAt" TIMESTAMP(3),
    "observedAt" TIMESTAMP(3) NOT NULL,

    CONSTRAINT "UsageReading_pkey" PRIMARY KEY ("id")
);

-- CreateTable
CREATE TABLE "Pool" (
    "id" TEXT NOT NULL,
    "accountKey" TEXT NOT NULL,
    "createdById" TEXT NOT NULL,
    "code" TEXT NOT NULL,
    "codeExpiresAt" TIMESTAMP(3) NOT NULL,
    "revokedAt" TIMESTAMP(3),
    "createdAt" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "Pool_pkey" PRIMARY KEY ("id")
);

-- CreateTable
CREATE TABLE "PoolMember" (
    "poolId" TEXT NOT NULL,
    "userId" TEXT NOT NULL,
    "joinedAt" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "PoolMember_pkey" PRIMARY KEY ("poolId","userId")
);

-- CreateTable
CREATE TABLE "PoolJoinFailure" (
    "id" BIGSERIAL NOT NULL,
    "userId" TEXT NOT NULL,
    "at" TIMESTAMP(3) NOT NULL,

    CONSTRAINT "PoolJoinFailure_pkey" PRIMARY KEY ("id")
);

-- CreateTable
CREATE TABLE "RateLimit" (
    "key" TEXT NOT NULL,
    "tokens" DOUBLE PRECISION NOT NULL,
    "updatedAt" TIMESTAMP(3) NOT NULL,

    CONSTRAINT "RateLimit_pkey" PRIMARY KEY ("key")
);

-- CreateIndex
CREATE INDEX "UserAccount_accountKey_idx" ON "UserAccount"("accountKey");

-- CreateIndex
CREATE INDEX "Project_accountKey_idx" ON "Project"("accountKey");

-- CreateIndex
CREATE UNIQUE INDEX "Project_userId_accountKey_key_key" ON "Project"("userId", "accountKey", "key");

-- CreateIndex
CREATE INDEX "Session_accountKey_startedAt_idx" ON "Session"("accountKey", "startedAt");

-- CreateIndex
CREATE INDEX "Session_userId_startedAt_idx" ON "Session"("userId", "startedAt");

-- CreateIndex
CREATE INDEX "Session_userId_createdAt_idx" ON "Session"("userId", "createdAt");

-- CreateIndex
CREATE INDEX "Session_projectId_idx" ON "Session"("projectId");

-- CreateIndex
CREATE UNIQUE INDEX "Session_userId_accountKey_sessionId_key" ON "Session"("userId", "accountKey", "sessionId");

-- CreateIndex
CREATE INDEX "UsageReading_accountKey_observedAt_idx" ON "UsageReading"("accountKey", "observedAt");

-- CreateIndex
CREATE UNIQUE INDEX "UsageReading_userId_accountKey_source_windowId_observedAt_key" ON "UsageReading"("userId", "accountKey", "source", "windowId", "observedAt");

-- CreateIndex
CREATE UNIQUE INDEX "Pool_code_key" ON "Pool"("code");

-- CreateIndex
CREATE INDEX "Pool_createdById_idx" ON "Pool"("createdById");

-- CreateIndex
CREATE UNIQUE INDEX "Pool_accountKey_createdById_key" ON "Pool"("accountKey", "createdById");

-- CreateIndex
CREATE INDEX "PoolMember_userId_idx" ON "PoolMember"("userId");

-- CreateIndex
CREATE INDEX "PoolJoinFailure_userId_at_idx" ON "PoolJoinFailure"("userId", "at");

-- AddForeignKey
ALTER TABLE "Device" ADD CONSTRAINT "Device_userId_fkey" FOREIGN KEY ("userId") REFERENCES "User"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "UserAccount" ADD CONSTRAINT "UserAccount_userId_fkey" FOREIGN KEY ("userId") REFERENCES "User"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "UserAccount" ADD CONSTRAINT "UserAccount_accountKey_fkey" FOREIGN KEY ("accountKey") REFERENCES "ClaudeAccount"("key") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "Project" ADD CONSTRAINT "Project_userId_fkey" FOREIGN KEY ("userId") REFERENCES "User"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "Project" ADD CONSTRAINT "Project_accountKey_fkey" FOREIGN KEY ("accountKey") REFERENCES "ClaudeAccount"("key") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "Session" ADD CONSTRAINT "Session_userId_fkey" FOREIGN KEY ("userId") REFERENCES "User"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "Session" ADD CONSTRAINT "Session_accountKey_fkey" FOREIGN KEY ("accountKey") REFERENCES "ClaudeAccount"("key") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "Session" ADD CONSTRAINT "Session_projectId_fkey" FOREIGN KEY ("projectId") REFERENCES "Project"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "Session" ADD CONSTRAINT "Session_userId_deviceId_fkey" FOREIGN KEY ("userId", "deviceId") REFERENCES "Device"("userId", "id") ON DELETE NO ACTION ON UPDATE NO ACTION;

-- AddForeignKey
ALTER TABLE "UsageReading" ADD CONSTRAINT "UsageReading_userId_fkey" FOREIGN KEY ("userId") REFERENCES "User"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "UsageReading" ADD CONSTRAINT "UsageReading_accountKey_fkey" FOREIGN KEY ("accountKey") REFERENCES "ClaudeAccount"("key") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "Pool" ADD CONSTRAINT "Pool_accountKey_fkey" FOREIGN KEY ("accountKey") REFERENCES "ClaudeAccount"("key") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "Pool" ADD CONSTRAINT "Pool_createdById_fkey" FOREIGN KEY ("createdById") REFERENCES "User"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "PoolMember" ADD CONSTRAINT "PoolMember_poolId_fkey" FOREIGN KEY ("poolId") REFERENCES "Pool"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "PoolMember" ADD CONSTRAINT "PoolMember_userId_fkey" FOREIGN KEY ("userId") REFERENCES "User"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "PoolJoinFailure" ADD CONSTRAINT "PoolJoinFailure_userId_fkey" FOREIGN KEY ("userId") REFERENCES "User"("id") ON DELETE CASCADE ON UPDATE CASCADE;


-- Row-level security (added by hand; Prisma does not model it).
-- Supabase's Data API serves every table in the public schema to the `anon` and `authenticated`
-- roles. The website reads and writes only through Prisma, connected as the tables' owner, which
-- RLS does not restrict; with RLS on and no policies, the Data API sees no rows at all.
ALTER TABLE "User" ENABLE ROW LEVEL SECURITY;
ALTER TABLE "Device" ENABLE ROW LEVEL SECURITY;
ALTER TABLE "ClaudeAccount" ENABLE ROW LEVEL SECURITY;
ALTER TABLE "UserAccount" ENABLE ROW LEVEL SECURITY;
ALTER TABLE "Project" ENABLE ROW LEVEL SECURITY;
ALTER TABLE "Session" ENABLE ROW LEVEL SECURITY;
ALTER TABLE "UsageReading" ENABLE ROW LEVEL SECURITY;
ALTER TABLE "Pool" ENABLE ROW LEVEL SECURITY;
ALTER TABLE "PoolMember" ENABLE ROW LEVEL SECURITY;
ALTER TABLE "PoolJoinFailure" ENABLE ROW LEVEL SECURITY;
ALTER TABLE "RateLimit" ENABLE ROW LEVEL SECURITY;
-- Prisma's own bookkeeping table sits in the same schema; it exists by the time `migrate deploy`
-- applies this file, but not in every shadow database, hence the guard.
DO $$
BEGIN
  IF to_regclass('"_prisma_migrations"') IS NOT NULL THEN
    EXECUTE 'ALTER TABLE "_prisma_migrations" ENABLE ROW LEVEL SECURITY';
  END IF;
END
$$;
