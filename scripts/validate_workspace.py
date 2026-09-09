#!/usr/bin/env python3
"""Generate and run a workspace app with a 100-file local Swift package."""
import argparse,json,shutil
from pathlib import Path
from mcp_client import Client
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--device',required=True);p.add_argument('--mx',default='target/release/mx');a=p.parse_args()
root=Path(__file__).resolve().parent.parent;out=root/'.mx/workspace-validation'
if out.exists():raise SystemExit('Output already exists: '+str(out))
shutil.copytree(root/'examples/MxDemo',out,ignore=shutil.ignore_patterns('.mx'))
pkg=out/'Support';src=pkg/'Sources/Support';src.mkdir(parents=True)
(pkg/'Package.swift').write_text('// swift-tools-version: 6.0\nimport PackageDescription\nlet package = Package(name: "Support", platforms: [.iOS(.v18)], products: [.library(name: "Support", targets: ["Support"])], targets: [.target(name: "Support")])\n')
for i in range(100):(src/f'Value{i}.swift').write_text(f'public enum Value{i} {{ public static let text = "Mx Demo" }}\n')
app=out/'AppDelegate.swift';app.write_text('import Support\n'+app.read_text().replace('"Mx Demo"','Value0.text'))
proj=out/'MxDemo.xcodeproj/project.pbxproj';s=proj.read_text()
s=s.replace('projectDirPath = "";', 'packageReferences = (ABC000000000000000000001); projectDirPath = "";')
s=s.replace('productType = "com.apple.product-type.application";', 'productType = "com.apple.product-type.application"; packageProductDependencies = (ABC000000000000000000002);')
s=s.replace('isa = PBXFrameworksBuildPhase; buildActionMask = 2147483647; files = ();','isa = PBXFrameworksBuildPhase; buildActionMask = 2147483647; files = (ABC000000000000000000003);')
s=s.replace('}; rootObject =', 'ABC000000000000000000001 = { isa = XCLocalSwiftPackageReference; relativePath = Support; };\nABC000000000000000000002 = { isa = XCSwiftPackageProductDependency; productName = Support; };\nABC000000000000000000003 = { isa = PBXBuildFile; productRef = ABC000000000000000000002; };\n}; rootObject =');proj.write_text(s)
w=out/'MxDemo.xcworkspace';w.mkdir();(w/'contents.xcworkspacedata').write_text('<?xml version="1.0"?><Workspace version="1.0"><FileRef location="group:MxDemo.xcodeproj"/></Workspace>')
c=Client(Path(a.mx).resolve());report={'fixture':'Generated workspace, local SwiftPM dependency with 100 small Swift source files. Not a production-scale benchmark.'};run=None
try:
    report['inspect']=c.call('mx_inspect',{'project':str(out)})
    run=c.call('mx_run',{'project':str(out),'scheme':'MxDemo','device':a.device,'inspect_ui':True});report['run']=run
    assert run['project'].endswith('.xcworkspace') and run['ui'] and not run['ui_error'],run
    assert any(e.get('label')=='Mx Demo' for e in run['ui']['elements']),run
    report['passed']=True
finally:
    if run:c.call('mx_stop',{'device':a.device,'bundle_id':run['bundle_id']})
    c.close();(root/'docs/profiling/workspace-validation.json').write_text(json.dumps(report,indent=2)+'\n')
print('Workspace and 100-file local Swift package passed.')
