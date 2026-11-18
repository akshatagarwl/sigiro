/** Minimal Workers host boundary used by the coordinator and its tests. */
export interface DurableObjectStorage {
  get<T>(key: string): Promise<T | undefined>;
  put<T>(key: string, value: T): Promise<void>;
  deleteAlarm(): Promise<void>;
  getAlarm(): Promise<number | null>;
  setAlarm(scheduledTime: number | Date): Promise<void>;
}

export interface DurableObjectState {
  readonly storage: DurableObjectStorage;
}

export interface DurableObjectId {}

export interface DurableObjectStub {
  fetch(request: Request): Promise<Response>;
}

export interface DurableObjectNamespace {
  idFromName(name: string): DurableObjectId;
  get(id: DurableObjectId): DurableObjectStub;
}

export interface CoordinatorEnv {
  readonly JOBS: DurableObjectNamespace;
  readonly QUEUE: DurableObjectNamespace;
  readonly WORKER_TOKEN: string;
}
