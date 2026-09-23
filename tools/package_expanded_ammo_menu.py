"""Package matching Core, regroup and expanded-menu DLLs; never touch the game."""
import hashlib, json, pathlib, subprocess, zipfile
ROOT=pathlib.Path(__file__).resolve().parent.parent

def main():
    entries=[]
    for build,manifest_dir,stem in [
        ("target/release","target/release","defiance_plugin_core"),
        ("plugins/regroup/target/release","plugins/regroup","defiance_plugin_regroup"),
        ("plugins/expanded-ammo-menu/target/release","plugins/expanded-ammo-menu","defiance_plugin_expanded_ammo_menu"),
    ]:
        dll=ROOT/build/(stem+".dll")
        pdb=dll.with_suffix(".pdb")
        manifest=ROOT/manifest_dir/(stem+".plugin.json")
        for path in (dll,pdb,manifest):
            if not path.is_file(): raise SystemExit(f"Missing {path}; build matching loader/plugins first")
        data=json.loads(manifest.read_text(encoding="utf-8"))
        enabled=next(s["default"] for s in data["settings"] if s["key"]=="enabled") if stem!="defiance_plugin_core" else ""
        if stem=="defiance_plugin_regroup": assert enabled=="false"
        if stem=="defiance_plugin_expanded_ammo_menu": assert enabled=="true"
        entries.append((dll,pdb,manifest))
    out=ROOT/"out"; out.mkdir(exist_ok=True)
    with zipfile.ZipFile(out/"expanded-ammo-regroup.zip","w",zipfile.ZIP_DEFLATED) as z:
        for dll,pdb,manifest in entries:
            for path in (dll,manifest): z.write(path,"DefianceLoader/plugins/"+path.name)
        z.write(ROOT/"plugins/expanded-ammo-menu/README.md","README.md")
        z.write(ROOT/"plugins/regroup/README.md","REGROUP.md")
    with zipfile.ZipFile(out/"expanded-ammo-regroup-symbols.zip","w",zipfile.ZIP_DEFLATED) as z:
        records=[]
        for dll,pdb,_ in entries:
            z.write(pdb,pdb.name)
            records.append({"dll":dll.name,"sha256":hashlib.sha256(dll.read_bytes()).hexdigest(),
                "pdb":pdb.name,"pdb_sha256":hashlib.sha256(pdb.read_bytes()).hexdigest()})
        z.writestr("build.json",json.dumps({
            "commit":subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT,text=True).strip(),
            "files":records},indent=2))
    print(out/"expanded-ammo-regroup.zip")
    print(out/"expanded-ammo-regroup-symbols.zip")
if __name__=="__main__":main()
