/**
 * Rate limiter with async/await support.
 * Limits concurrent execution of async functions.
 */

export interface LimitOptions {
  concurrency: number;
  timeout?: number;
}

export class RateLimiter {
  private activeCount = 0;
  private queue: Array<() => void> = [];

  constructor(private opts: LimitOptions) {}

  async run<T>(fn: () => Promise<T>): Promise<T> {
    await this.waitForSlot();
    this.activeCount++;

    try {
      const result = await this.withTimeout(fn());
      return result;
    } finally {
      this.activeCount--;
      this.next();
    }
  }

  private async waitForSlot(): Promise<void> {
    if (this.activeCount < this.opts.concurrency) {
      return;
    }

    return new Promise<void>((resolve) => {
      this.queue.push(resolve);
    });
  }

  private next(): void {
    if (this.queue.length > 0) {
      const resolve = this.queue.shift()!;
      resolve();
    }
  }

  private async withTimeout<T>(promise: Promise<T>): Promise<T> {
    if (!this.opts.timeout) return promise;

    return Promise.race([
      promise,
      new Promise<T>((_, reject) =>
        setTimeout(() => reject(new Error('Timeout')), this.opts.timeout)
      ),
    ]);
  }
}
