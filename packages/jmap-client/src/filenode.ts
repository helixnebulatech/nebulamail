// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// JMAP FileNode (draft-ietf-jmap-filenode-18, as Stalwart implements it):
// files and folders. Only what the app needs to keep a small file of its
// own (synced preferences). Checked live against Stalwart:
//
// - The kind is `@type` ("File", "Directory", "SymLink"), required on
//   create; a file has `blobId` and `mediaType`. An unknown key fails the
//   create with `invalidProperties`. `parentId: null` is the top level.
// - `#creationId` references work across method calls of one request.
// - Filters `isTopLevel`, `parentId` and `name` (exact) work; updating a
//   file is setting its `blobId`; `onDestroyRemoveChildren` deletes a
//   folder with its contents.

// See ids.ts for why each id is a schema and a type with the same name.
/* oxlint-disable no-redeclare */

import { z } from "zod";

import { CAPABILITIES } from "./capabilities";
import { parseOrThrow } from "./errors";
import { BlobId } from "./ids";
import {
  getResponseSchema,
  queryResponseSchema,
  setResponseSchema,
} from "./standard";
import type {
  GetArgs,
  GetResponse,
  Lenient,
  QueryArgs,
  QueryResponse,
  SetArgs,
  SetResponse,
} from "./standard";

export const FileNodeId = z
  .string()
  .min(1)
  .max(255)
  .regex(/^[\w-]+$/u, "Not a valid JMAP Id")
  .brand<"FileNodeId">();
export type FileNodeId = z.infer<typeof FileNodeId>;

export const FileNodeSchema = z.looseObject({
  /** "File", "Directory" or "SymLink". */
  "@type": z.string().nullish(),
  blobId: BlobId.nullish(),
  id: FileNodeId,
  mediaType: z.string().nullish(),
  modified: z.string().nullish(),
  name: z.string(),
  parentId: FileNodeId.nullish(),
  size: z.number().nullish(),
});
export type FileNode = z.infer<typeof FileNodeSchema>;

export type FileNodeProperty = keyof FileNode;

/** What /get and /set return: the asked-for properties and the id. */
export type FileNodeFound = Lenient<FileNode> & { readonly id: FileNodeId };

export type FileNodeGetArgs = GetArgs<FileNodeId, FileNodeProperty>;

/** Build it with `fileNodeCreate`. */
export interface FileNodeCreate {
  readonly "@type": "Directory" | "File";
  readonly name: string;
  /** Id, `#creationId`, or `null` for the top level. */
  readonly parentId: string | null;
  /** Omitted for a folder. */
  readonly blobId?: BlobId;
  /** Omitted for a folder. */
  readonly mediaType?: string;
}

export interface FileNodePatch {
  readonly name?: string;
  readonly blobId?: BlobId;
  readonly mediaType?: string;
}

/** The create object for a folder or a file. */
export const fileNodeCreate = (
  node:
    | {
        readonly kind: "directory";
        readonly name: string;
        readonly parentId: string | null;
      }
    | {
        readonly kind: "file";
        readonly name: string;
        readonly parentId: string | null;
        readonly blobId: BlobId;
        readonly mediaType: string;
      }
): FileNodeCreate =>
  node.kind === "directory"
    ? { "@type": "Directory", name: node.name, parentId: node.parentId }
    : {
        "@type": "File",
        blobId: node.blobId,
        mediaType: node.mediaType,
        name: node.name,
        parentId: node.parentId,
      };

export interface FileNodeSetArgs extends SetArgs<
  FileNodeId,
  FileNodeCreate,
  FileNodePatch
> {
  /** Delete a folder's contents with it (otherwise it must be empty). */
  readonly onDestroyRemoveChildren?: boolean;
}

export interface FileNodeFilterCondition {
  readonly isTopLevel?: boolean;
  readonly parentId?: FileNodeId;
  /** Exact name. */
  readonly name?: string;
}

export type FileNodeQueryArgs = QueryArgs<
  FileNodeFilterCondition,
  { readonly property: "name" | "size"; readonly isAscending?: boolean }
>;

interface Spec<A, R> {
  readonly args: A;
  readonly response: R;
}

/** Merged into the method registry (methods.ts). */
export interface FileNodeMethodMap {
  "FileNode/get": Spec<FileNodeGetArgs, GetResponse<FileNodeFound, FileNodeId>>;
  "FileNode/query": Spec<FileNodeQueryArgs, QueryResponse<FileNodeId>>;
  "FileNode/set": Spec<FileNodeSetArgs, SetResponse<FileNodeFound, FileNodeId>>;
}

const parser =
  <T>(schema: z.ZodType<T>) =>
  (_args: unknown, raw: Record<string, unknown>): T =>
    parseOrThrow(schema, raw);

const USING = [CAPABILITIES.fileNode];

const FoundSchema = FileNodeSchema.partial().extend({ id: FileNodeId });

export const FILENODE_METHODS = {
  "FileNode/get": {
    parse: parser(getResponseSchema(FoundSchema, FileNodeId)),
    using: USING,
  },
  "FileNode/query": {
    parse: parser(queryResponseSchema(FileNodeId)),
    using: USING,
  },
  "FileNode/set": {
    parse: parser(
      setResponseSchema(FoundSchema, FileNodeId, FileNodeSchema.partial())
    ),
    using: USING,
  },
} as const;
