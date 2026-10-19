//! Optional fixed-input service activation stage; never launches a separate VM.
use super::*;
use crate::{profile_bundle, service_fixture};

pub(super) struct Inputs {
    pub paths: [PathBuf; 3],
    pub hashes: [String; 3],
}
pub(super) fn freeze(sources: [PathBuf; 3], run: &Path) -> Result<Inputs> {
    let names = ["input-service-fixture", "input-user-service", "input-root-helper"];
    let mut paths = Vec::new();
    let mut hashes = Vec::new();
    for (source, name) in sources.iter().zip(names) {
        let path = stage_input_binary(source, &run.join(name))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o500))?;
        crate::preflight::input_binary(path.to_str().ok_or("invalid service input path")?)?;
        hashes.push(profile_bundle::digest(&path)?);
        paths.push(path);
    }
    Ok(Inputs {
        paths: paths.try_into().map_err(|_| "missing frozen service input")?,
        hashes: hashes.try_into().map_err(|_| "missing frozen service hash")?,
    })
}
impl Lab {
    fn service_command(&self, request: &str, log: &str) -> Result<String> {
        let result = self.ssh(request);
        save_private_log(&self.run, log, &fs::read(self.run.join("guest-private.log"))?)?;
        result
    }
    pub(super) fn service_fixture(&mut self) -> Result<()> {
        let inputs = self.service_fixture.as_ref().ok_or("missing service fixture inputs")?;
        for ((path, hash), name) in
            inputs.paths.iter().zip(&inputs.hashes).zip(service_fixture::NAMES)
        {
            require(
                profile_bundle::digest(path)? == *hash,
                "service input changed after launch freeze",
            )?;
            let mut copy = Command::new("scp");
            copy.args(["-F", "/dev/null", "-i"])
                .arg(self.run.join("id_ed25519"))
                .args([
                    "-P",
                    &self.port.ok_or("owned SSH endpoint unknown")?.to_string(),
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "StrictHostKeyChecking=yes",
                    "-o",
                ])
                .arg(format!("UserKnownHostsFile={}", self.run.join("known_hosts").display()))
                .arg(path)
                .arg(format!("tester@127.0.0.1:/tmp/{name}.upload"));
            self.run_private_child(
                &mut copy,
                &format!("{name}-copy-private.log"),
                "SERVICE_INPUT_COPY_TIMEOUT",
                "SERVICE_INPUT_COPY_FAILED",
            )?;
        }
        let stage = self.service_command(
            &format!(
                "sudo /tmp/ryoku-vm-lab guest-service-stage {} {} {}",
                inputs.hashes[0], inputs.hashes[1], inputs.hashes[2]
            ),
            "service-stage-private.log",
        )?;
        require(
            stage == "native_service_guest_artifacts=verified\n",
            "service guest input staging incomplete",
        )?;
        self.evidence(&stage)?;
        let preflight = self.service_command(
            &format!("sudo {} --check", service_fixture::PROBE),
            "service-preflight-private.log",
        )?;
        service_fixture::validate(&preflight, false)?;
        let result = self.service_command(
            &format!("sudo {} --run", service_fixture::PROBE),
            "service-fixture-private.log",
        )?;
        self.evidence(&service_fixture::validate(&result, true)?)
    }
}
