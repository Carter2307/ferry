//! Blueprints: declarative `ferry.yaml` / `render.yaml` files (DESIGN.md §Blueprints).
//!
//! `parse` turns YAML into a typed [`Blueprint`]; `plan`/`apply` diff it
//! against the store and create/update env groups, datastores and services
//! (never deleting anything), then queue deploys for new or changed services.
