export type Status = "pending" | "active" | "completed" | "failed";

export enum Priority {
  Low = 0,
  Medium = 1,
  High = 2,
  Critical = 3,
}

export interface Task {
  id: string;
  title: string;
  description: string;
  status: Status;
  priority: Priority;
  createdAt: Date;
  updatedAt: Date;
}

export const createTask = (title: string, description: string): Task => ({
  id: crypto.randomUUID(),
  title,
  description,
  status: "pending",
  priority: Priority.Medium,
  createdAt: new Date(),
  updatedAt: new Date(),
});

export const updateTaskStatus = (task: Task, status: Status): Task => ({
  ...task,
  status,
  updatedAt: new Date(),
});
