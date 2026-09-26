-- Per-IP limit on failed share-code redemptions: one row per failure, keyed by a hash of the
-- client's IP address (never the address) and naming no user. Kept for an hour, like
-- "PoolJoinFailure", which holds the per-user half of the limit.

-- CreateTable
CREATE TABLE "PoolJoinIpFailure" (
    "id" BIGSERIAL NOT NULL,
    "ipKey" TEXT NOT NULL,
    "at" TIMESTAMP(3) NOT NULL,

    CONSTRAINT "PoolJoinIpFailure_pkey" PRIMARY KEY ("id")
);

-- CreateIndex
CREATE INDEX "PoolJoinIpFailure_ipKey_at_idx" ON "PoolJoinIpFailure"("ipKey", "at");

-- Row-level security, as for every table (see 0001_init).
ALTER TABLE "PoolJoinIpFailure" ENABLE ROW LEVEL SECURITY;
