import { LoadingBlock, Skeleton } from "../../_components/ui";

export default function ProjectLoading() {
  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-10 px-4 py-8 sm:py-10">
      <LoadingBlock
        label="Loading the project…"
        className="flex flex-col gap-5"
      >
        <Skeleton className="h-4 w-40" />
        <div className="flex flex-col gap-2">
          <Skeleton className="h-8 w-72 max-w-full" />
          <Skeleton className="h-4 w-56 max-w-full" />
        </div>
        <div
          className="card grid grid-cols-2 gap-5 p-5 sm:grid-cols-4"
          aria-hidden="true"
        >
          {[0, 1, 2, 3].map((i) => (
            <div key={i} className="flex flex-col gap-2">
              <Skeleton className="h-3 w-24" />
              <Skeleton className="h-6 w-16" />
            </div>
          ))}
        </div>
        <div className="card flex flex-col gap-5 p-5" aria-hidden="true">
          {[0, 1].map((i) => (
            <div key={i} className="flex flex-col gap-2">
              <Skeleton className="h-4 w-48 max-w-1/2" />
              <Skeleton className="h-2 w-full rounded-full" />
            </div>
          ))}
        </div>
      </LoadingBlock>
    </div>
  );
}
