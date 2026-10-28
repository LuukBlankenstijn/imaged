use dioxus::prelude::*;

use crate::model::{
    Image, ImageScriptsRequest, RootPartitionRequest, Script, ScriptRequest, ScriptRootRequest,
    UpdateScriptRequest,
};

#[cfg(feature = "server")]
use injectable::inject;

#[cfg(feature = "server")]
use imaged_core::di::{ImageRepo, ScriptRepo, ScriptVariables};
use imaged_model::error::Result;

#[get("/api/ui/scripts")]
#[inject(script_repo: ScriptRepo)]
pub async fn get_all_scripts() -> Result<Vec<Script>> {
    let scripts = script_repo.get_all().await?;
    Ok(scripts.into_iter().map(Into::into).collect())
}

#[get("/api/ui/scripts/variables")]
#[inject(script_variables: ScriptVariables)]
pub async fn get_script_variable_names() -> Result<Vec<String>> {
    Ok(script_variables.keys().cloned().collect())
}

#[post("/api/ui/scripts/create")]
#[inject(script_repo: ScriptRepo)]
pub async fn create_script(req: ScriptRequest) -> Result<Script> {
    let script = script_repo.create(&req.into()).await?;
    Ok(script.into())
}

#[post("/api/ui/scripts/update")]
#[inject(script_repo: ScriptRepo)]
pub async fn update_script(req: UpdateScriptRequest) -> Result<Script> {
    let script = script_repo.update(req.id, &req.script.into()).await?;
    Ok(script.into())
}

#[post("/api/ui/scripts/delete")]
#[inject(script_repo: ScriptRepo)]
pub async fn delete_script(id: i64) -> Result<()> {
    script_repo.delete(id).await?;
    Ok(())
}

#[get("/api/ui/images/{id}/scripts")]
#[inject(script_repo: ScriptRepo)]
pub async fn get_image_scripts(id: i64) -> Result<Vec<Script>> {
    let scripts = script_repo.get_for_image(id).await?;
    Ok(scripts.into_iter().map(Into::into).collect())
}

#[post("/api/ui/images/scripts")]
#[inject(script_repo: ScriptRepo)]
pub async fn set_image_scripts(req: ImageScriptsRequest) -> Result<()> {
    script_repo
        .set_for_image(req.image_id, &req.script_ids)
        .await?;
    Ok(())
}

#[post("/api/ui/images/scripts/root")]
#[inject(script_repo: ScriptRepo)]
pub async fn set_script_root_partition(req: ScriptRootRequest) -> Result<()> {
    script_repo
        .set_root_for_image(req.image_id, req.position, req.partition_number)
        .await
}

#[post("/api/ui/images/root-partition")]
#[inject(image_repo: ImageRepo)]
pub async fn set_root_partition(req: RootPartitionRequest) -> Result<Image> {
    let image = image_repo
        .set_root_partition(req.image_id, req.partition_number)
        .await?;
    Ok(image.into())
}
