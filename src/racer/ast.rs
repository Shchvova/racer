extern crate syntax;
use syntax::ast;
use syntax::parse::{new_parse_sess};
use syntax::parse::{ParseSess};
use syntax::parse::{new_parser_from_source_str};
use syntax::parse::parser::Parser;
use syntax::parse::token;
use syntax::visit;
use syntax::codemap;
use std::gc::Gc;
use std::task;
use racer::Match;
use racer;
use racer::util;
use racer::nameres::{resolve_path_with_str};
use racer::typeinf;
use syntax::visit::Visitor;
use racer::nameres;

use std::io::{BufferedReader, File};

#[deriving(Clone)]
struct Scope {
    pub filepath: Path,
    pub point: uint
}

// This code ripped from libsyntax::util::parser_testing
pub fn string_to_parser<'a>(ps: &'a ParseSess, source_str: String) -> Parser<'a> {
    new_parser_from_source_str(ps,
                               Vec::new(),
                               "bogofile".to_string(),
                               source_str)
}

// pub fn string_to_parser<'a>(ps: &'a ParseSess, source_str: ~str) -> Parser<'a> {
//     new_parser_from_source_str(ps, Vec::new(), "bogofile".to_owned(), source_str)
// }

fn with_error_checking_parse<T>(s: String, f: |&mut Parser| -> T) -> T {
    let ps = new_parse_sess();
    let mut p = string_to_parser(&ps, s);
    let x = f(&mut p);
    p.abort_if_errors();
    x
}

// parse a string, return an expr
pub fn string_to_expr (source_str : String) -> Gc<ast::Expr> {
    with_error_checking_parse(source_str, |p| {
        p.parse_expr()
    })
}

// parse a string, return an item
pub fn string_to_item (source_str : String) -> Option<Gc<ast::Item>> {
    with_error_checking_parse(source_str, |p| {
        p.parse_item(Vec::new())
    })
}

// parse a string, return a stmt
pub fn string_to_stmt(source_str : String) -> Gc<ast::Stmt> {
    with_error_checking_parse(source_str, |p| {
        p.parse_stmt(Vec::new())
    })
}

// parse a string, return a crate.
pub fn string_to_crate (source_str : String) -> ast::Crate {
    with_error_checking_parse(source_str, |p| {
        p.parse_crate_mod()
    })
}

pub struct ViewItemVisitor {
    pub ident : Option<String>,
    pub paths : Vec<Vec<String>>
}

impl visit::Visitor<()> for ViewItemVisitor {
    fn visit_view_item(&mut self, i: &ast::ViewItem, e: ()) { 
        match i.node {
            ast::ViewItemUse(ref path) => {
                match path.node {
                    ast::ViewPathSimple(ident, ref path, _) => {
                        let mut v = Vec::new();
                        for seg in path.segments.iter() {
                            v.push(token::get_ident(seg.identifier).get().to_string())
                        }
                        self.paths.push(v);
                        self.ident = Some(token::get_ident(ident).get().to_string());
                    },
                    ast::ViewPathList(ref pth, ref paths, _) => {
                        let mut v = Vec::new();

                        for seg in pth.segments.iter() {
                            v.push(token::get_ident(seg.identifier).get().to_string())
                        }

                        for path in paths.iter() {
                            //debug!("PHIL view path list item {}",token::get_ident(path.node.name));
                            match path.node {
                                ast::PathListIdent{name, ..} => {
                                    let mut vv = v.clone();
                                    vv.push(token::get_ident(name).get().to_string());
                                    self.paths.push(vv);
                                }
                                ast::PathListMod{..} => (), // TODO
                            }
                        }
                    }
                    ast::ViewPathGlob(_, id) => {
                        debug!("PHIL got glob {:?}",id);
                    }
                }
            },
            ast::ViewItemExternCrate(ident, ref loc, _) => {
                self.ident = Some(token::get_ident(ident).get().to_string());

                let ll = loc.clone();
                ll.map(|(ref istr, _ /* str style */)| {
                    let mut v = Vec::new();
                    v.push(istr.get().to_string());
                    self.paths.push(v);
                });
            }
        }

        visit::walk_view_item(self, i, e) 
    }
}

struct LetVisitor {
    scope: Scope,
    need_type: bool,
    result: Option<LetResult>
}

pub struct LetResult { 
    pub name: String,
    pub point: uint,
    pub inittype: Option<Match>
}


fn resolve_ast_path(path: &ast::Path, filepath: &Path, pos: uint) -> Option<Match> {
    let mut it = path.segments.iter();
    
    let seg = it.next().unwrap();
    
    let name = token::get_ident(seg.identifier).get().to_string();
    let mut context = nameres::resolve_name(name.as_slice(), 
                                            filepath, pos, 
                                            racer::ExactMatch, 
                                            racer::BothNamespaces).nth(0);
    for seg in it {

        let m = match context { Some(ref m) => m.clone(), None => break };
        match m.mtype {
            racer::Module => {
                debug!("PHIL searching a module '{}' (whole path: {})",m.matchstr, path);
                
                let name = token::get_ident(seg.identifier).get().to_string();
                

                // let searchstr = path[path.len()-1];
                for m in nameres::search_next_scope(m.point, name.as_slice(), &m.filepath, racer::ExactMatch, false, racer::BothNamespaces).nth(0).move_iter() { 
                    context = Some(m);
                }
            }
            racer::Struct => {
                debug!("PHIL found a struct. Now need to look for impl");
                let name = token::get_ident(seg.identifier).get().to_string();
                for m in nameres::search_for_impls(m.point, m.matchstr.as_slice(), &m.filepath, m.local, false) {
                    debug!("PHIL found impl!! {}",m);

                    let filetxt = BufferedReader::new(File::open(&m.filepath)).read_to_end().unwrap();
                    let src = ::std::str::from_utf8(filetxt.as_slice()).unwrap();
                    
                    // find the opening brace and skip to it. 
                    src.slice_from(m.point).find_str("{").map(|n|{
                        let point = m.point + n + 1;
                        for m in nameres::search_scope(point, src, name.as_slice(), &m.filepath, racer::ExactMatch, m.local, racer::BothNamespaces).nth(0).move_iter() {
                            context = Some(m);
                        }
                    });
                    
                };
            }
            _ => {context = None; break}
        }
    }

    debug!("PHIL resolve_ast_path returning {}", context);
    return context;

}

fn path_to_vec(pth: &ast::Path) -> Vec<String> {
    let mut v = Vec::new();
    for seg in pth.segments.iter() {
        v.push(token::get_ident(seg.identifier).get().to_string());
    }
    return v;
}


struct ExprTypeVisitor {
    scope: Scope,
    result: Option<Match>
}

fn find_match(fqn: &Vec<String>, fpath: &Path, pos: uint) -> Option<Match> {
    let myfqn = util::to_refs(fqn);  
    return resolve_path_with_str(myfqn.as_slice(), fpath, pos, racer::ExactMatch,
        racer::BothNamespaces).nth(0);
}

fn find_type_match(fqn: &Vec<String>, fpath: &Path, pos: uint) -> Option<Match> {
    let myfqn = util::to_refs(fqn);  
    return resolve_path_with_str(myfqn.as_slice(), fpath, pos, racer::ExactMatch, 
               racer::TypeNamespace).nth(0).and_then(|m| {
                   match m.mtype {
                       racer::Type => get_type_of_typedef(m),
                       _ => Some(m)
                   }
               });
}

fn get_type_of_typedef(m: Match) -> Option<Match> {
    debug!("PHIL get_type_of_typedef match is {}",m);
    let msrc = racer::load_file_and_mask_comments(&m.filepath);
    let blobstart = m.point - 5;  // - 5 because 'type '
    let blob = msrc.as_slice().slice_from(blobstart);

    return racer::codeiter::iter_stmts(blob).nth(0).and_then(|(start, end)|{
        let blob = msrc.as_slice().slice(blobstart + start, blobstart+end).to_string();
        debug!("PHIL get_type_of_typedef blob string {}",blob);
        let res = parse_type(blob);
        debug!("PHIL get_type_of_typedef parsed type {}",res.type_);
        return Some(res.type_);
    }).and_then(|fqn|{
        let fqn = util::to_refs(&fqn);
        nameres::resolve_path_with_str(fqn.as_slice(), &m.filepath, m.point, racer::ExactMatch, racer::TypeNamespace).nth(0)
    });
}

fn get_type_of_path(fqn: &Vec<String>, fpath: &Path, pos: uint) -> Option<Match> {
    let om = find_match(fqn, fpath, pos);
    if om.is_some() {
        let m = om.unwrap();
        let msrc = racer::load_file_and_mask_comments(&m.filepath);
        return typeinf::get_type_of_match(m, msrc.as_slice());
    } else {
        return None;
    }
}

impl visit::Visitor<()> for ExprTypeVisitor {
    fn visit_expr(&mut self, expr: &ast::Expr, _: ()) { 
        debug!("PHIL visit_expr {:?}",expr);
        //walk_expr(self, ex, e) 
        match expr.node {
            ast::ExprPath(ref path) => {
                debug!("PHIL expr is a path {}",path_to_vec(path));
                self.result = resolve_ast_path(path, 
                                 &self.scope.filepath, 
                                 self.scope.point).and_then(|m| {
                   let msrc = racer::load_file_and_mask_comments(&m.filepath);
                   typeinf::get_type_of_match(m, msrc.as_slice())
                                 });
            }
            ast::ExprCall(callee_expression, _/*ref arguments*/) => {
                self.visit_expr(&*callee_expression, ());
                let mut newres: Option<Match> = None;
                {
                    let res = &self.result;
                    match *res {
                        Some(ref m) => {
                            let fqn = racer::typeinf::get_return_type_of_function(m);
                            debug!("PHIL found exprcall return type: {}",fqn);
                            newres = find_type_match(&fqn, &m.filepath, m.point);
                        },
                        None => {}
                    }
                }
                self.result = newres;
            }
            ast::ExprStruct(ref path, _, _) => {
                let pathvec = path_to_vec(path);
                self.result = find_type_match(&pathvec,
                                         &self.scope.filepath,
                                         self.scope.point);
            }

            ast::ExprMethodCall(ref spannedident, ref types, ref arguments) => {
                // spannedident.node is an ident I think
                let methodname = token::get_ident(spannedident.node).get().to_string();
                debug!("PHIL method call ast name {}",methodname);
                debug!("PHIL method call ast types {:?} {}",types, types.len());
                
                let objexpr = arguments[0];
                //println!("PHIL obj expr is {:?}",objexpr);
                self.visit_expr(&*objexpr, ());
                let mut newres: Option<Match> = None;
                match self.result {
                    Some(ref m) => {
                        debug!("PHIL obj expr type is {}",m);

                        // locate the method
                        let omethod = nameres::search_for_impl_methods(
                            m.matchstr.as_slice(),
                            methodname.as_slice(), 
                            m.point, 
                            &m.filepath,
                            m.local,
                            racer::ExactMatch).nth(0);

                        match omethod {
                            Some(ref m) => {
                            let fqn = racer::typeinf::get_return_type_of_function(m);
                            debug!("PHIL found exprcall return type: {}",fqn);
                            newres = find_type_match(&fqn, &m.filepath, m.point);
                            
                            }
                            None => {}
                        }
                    }
                    None => {}
                }
                debug!("PHIL at end of method call, type is {}",newres);
                self.result = newres;
            }

            ast::ExprField(subexpression, spannedident, _) => {
                let fieldname = token::get_ident(spannedident.node).get().to_string();
                self.visit_expr(&*subexpression, ());
                
                // if expr is a field, parent is a struct (at the moment)
                let m = self.result.clone();

                let newres = m.and_then(|m|{
                    typeinf::get_struct_field_type(fieldname.as_slice(), &m)
                });

                self.result = newres;
            }

            _ => {
                println!("PHIL - Could not match expr node type: {:?}",expr.node);
            }
        }
    }

}

impl visit::Visitor<()> for LetVisitor {

    fn visit_decl(&mut self, decl: &ast::Decl, e: ()) { 
        debug!("PHIL visit_decl {:?}",decl);
        match decl.node {
            ast::DeclLocal(local) => {
                debug!("PHIL visit_decl is a DeclLocal {:?}",local);
                match local.pat.node {
                    ast::PatIdent(_ , ref spannedident, _) => {
                        debug!("PHIL visit_decl is a patIdent {:?}",local);
                        let codemap::BytePos(point) = spannedident.span.lo;
                        let name = token::get_ident(spannedident.node).get().to_string();


                        self.result = Some(LetResult{name: name.to_string(),
                                                     point: point as uint,
                                                     inittype: None});
                        if !self.need_type {
                            // don't need the type. All done
                            debug!("PHIL visit_decl don't need the type. returning");
                            
                            return;
                        }

                        // Parse the type
                        debug!("PHIL visit_decl local.ty.node {:?}", local.ty.node);

                        let typepath = match local.ty.node {
                            ast::TyRptr(_, ref ty) => {
                                match ty.ty.node {
                                    ast::TyPath(ref path, _, _) => {
                                        let type_ = path_to_vec(path);
                                        debug!("PHIL struct field type is {}", type_);
                                        type_
                                    }
                                    _ => Vec::new()
                                }
                            }
                            ast::TyPath(ref path, _, _) => {
                                let type_ = path_to_vec(path);
                                debug!("PHIL struct field type is {}", type_);
                                type_
                            }
                            _ => {
                                Vec::new()  
                            }
                        };
                        
                        if !typepath.is_empty() {
                            debug!("PHIL visit_decl typepath is {}", typepath);
                            let m = get_type_of_path(&typepath,
                                                     &self.scope.filepath,
                                                     self.scope.point);
                            debug!("PHIL visit_decl type of path {}", m);
                            self.result = Some(LetResult{name: name.to_string(), 
                                                         point: point as uint,
                                                         inittype: m});
                            return;
                        }

                        debug!("PHIL result before is {:?}",self.result);

                        // That didn't work. Attempt to parse the init
                        local.init.map(|initexpr| {
                            debug!("PHIL init node is {:?}",initexpr.node);

                            let mut v = ExprTypeVisitor{ scope: self.scope.clone(),
                                                         result: None};
                            v.visit_expr(&*initexpr, ());

                            self.result = Some(LetResult{name: name.to_string(), 
                                                         point: point as uint,
                                                         inittype: v.result});

                        });

                    },
                    _ => {}
                }

                
            }
            ast::DeclItem(_) => {
                println!("PHIL WARN visit_decl is a DeclItem. Not handled");
            }
        }

        visit::walk_decl(self, decl, e);
    }

}

struct StructVisitor {
    pub fields: Vec<(String, uint, Vec<String>)>
}

impl visit::Visitor<()> for StructVisitor {
    fn visit_struct_def(&mut self, struct_definition: &ast::StructDef, _: ast::Ident, _: &ast::Generics, _: ast::NodeId, _: ()) {

        for field in struct_definition.fields.iter() {
            let codemap::BytePos(point) = field.span.lo;

            match field.node.kind {
                ast::NamedField(name, _) => {
                    //visitor.visit_ident(struct_field.span, name, env.clone())
                    let n = String::from_str(token::get_ident(name).get());
                    

                    // we have the name. Now get the type
                    let typepath = match field.node.ty.node {
                        ast::TyRptr(_, ref ty) => {
                            match ty.ty.node {
                                ast::TyPath(ref path, _, _) => {
                                    let type_ = path_to_vec(path);
                                    debug!("PHIL struct field type is {}", type_);
                                    type_
                                }
                                _ => Vec::new()
                            }
                        }
                        ast::TyPath(ref path, _, _) => {
                            let type_ = path_to_vec(path);
                            debug!("PHIL struct field type is {}", type_);
                            type_
                        }
                        _ => {
                            Vec::new()  
                        }
                    };

                    self.fields.push((n, point as uint, typepath));
                }
                _ => {}
            }



        }
    }
}

pub struct TypeVisitor {
    pub name: Option<String>,
    pub type_: Vec<String>
}

impl visit::Visitor<()> for TypeVisitor {
    fn visit_item(&mut self, item: &ast::Item, _: ()) { 
        match item.node {
            ast::ItemTy(ty, _) => {
                self.name = Some(token::get_ident(item.ident).get().to_string());

                let typepath = match ty.node {
                    ast::TyRptr(_, ref ty) => {
                        match ty.ty.node {
                            ast::TyPath(ref path, _, _) => {
                                let type_ = path_to_vec(path);
                                debug!("PHIL type type is {}", type_);
                                type_
                            }
                            _ => Vec::new()
                        }
                    }
                    ast::TyPath(ref path, _, _) => {
                        let type_ = path_to_vec(path);
                        debug!("PHIL type type is {}", type_);
                        type_
                    }
                    _ => {
                        Vec::new()  
                    }
                };
                self.type_ = typepath;
                debug!("PHIL typevisitor type is {}",self.type_);
            }
            _ => ()
        }
    }
}

pub struct TraitVisitor {
    pub name: Option<String>
}

impl visit::Visitor<()> for TraitVisitor {
    fn visit_item(&mut self, item: &ast::Item, _: ()) { 
        match item.node {
            ast::ItemTrait(_, _, _, _) => {
                self.name = Some(token::get_ident(item.ident).get().to_string());
            }
            _ => ()
        }
    }
}

pub struct ImplVisitor {
    pub name_path: Vec<String>,
    pub trait_path: Vec<String>
}

impl visit::Visitor<()> for ImplVisitor {
    fn visit_item(&mut self, item: &ast::Item, _: ()) { 
        match item.node {
            ast::ItemImpl(_,ref otrait, typ,_) => { 
                match typ.node {
                    ast::TyPath(ref path, _, _) => {
                        self.name_path = path_to_vec(path);
                    }
                    ast::TyRptr(_, ref ty) => {
                        // HACK for now, treat refs the same as unboxed types 
                        // so that we can match '&str' to 'str'
                        match ty.ty.node {
                            ast::TyPath(ref path, _, _) => {
                                self.name_path = path_to_vec(path);
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
                otrait.as_ref().map(|ref t|{
                    self.trait_path = path_to_vec(&t.path);
                });

            },
            _ => {}
        }
    }
}

pub struct FnVisitor {
    pub name: String,
    pub output: Vec<String>,
    pub args: Vec<(String, uint, Vec<String>)>,
    pub is_method: bool
}

impl visit::Visitor<()> for FnVisitor {
    fn visit_fn(&mut self, fk: &visit::FnKind, fd: &ast::FnDecl, _: &ast::Block, _: codemap::Span, _: ast::NodeId, _: ()) {

        self.name = token::get_ident(visit::name_of_fn(fk)).get().to_string();

        for arg in fd.inputs.iter() {
            debug!("PHIL fn arg ast is {:?}",arg);
            let res  = 
                match arg.pat.node {
                    ast::PatIdent(_ , ref spannedident, _) => {
                        let codemap::BytePos(point) = spannedident.span.lo;
                        let argname = token::get_ident(spannedident.node).get().to_string();
                        Some((String::from_str(argname.as_slice()), point as uint))
                    }
                    _ => None
                };

            if res.is_none() {
                return;
            }

            let (name, pos) = res.unwrap();

            let typepath = match arg.ty.node {
                ast::TyRptr(_, ref ty) => {
                    match ty.ty.node {
                        ast::TyPath(ref path, _, _) => {
                            let type_ = path_to_vec(path);
                            debug!("PHIL arg type is {}", type_);
                            type_
                        }
                        _ => Vec::new()
                    }
                }
                ast::TyPath(ref path, _, _) => {
                    let type_ = path_to_vec(path);
                    debug!("PHIL arg type is {}", type_);
                    type_
                }
                _ => Vec::new()  
            };

            debug!("PHIL typepath {}", typepath);
            self.args.push((name, pos, typepath))
        }

        debug!("PHIL parsed args: {}", self.args);

        match fd.output.node {
            ast::TyPath(ref path, _, _) => {
                self.output = path_to_vec(path);
            }
            _ => {}
        }

        self.is_method = match *fk {
            visit::FkMethod(_, _, _) => true,
            _ => false
        }
    }

}

pub struct ModVisitor {
    pub name: Option<String>
}

impl visit::Visitor<()> for ModVisitor {
    fn visit_item(&mut self, item: &ast::Item, _: ()) { 
        match item.node {
            ast::ItemMod(_) => {
                self.name = Some(String::from_str(token::get_ident(item.ident).get()));
            }
            _ => {}
        }
    }

}


pub struct EnumVisitor {
    pub name: String,
    pub values: Vec<(String, uint)>,
}

impl visit::Visitor<()> for EnumVisitor {
    fn visit_item(&mut self, i: &ast::Item, _: ()) { 
        match i.node {
            ast::ItemEnum(ref enum_definition, _) => {
                self.name = String::from_str(token::get_ident(i.ident).get());
                //visitor.visit_generics(type_parameters, env.clone());
                //visit::walk_enum_def(self, enum_definition, type_parameters, e)

                let codemap::BytePos(point) = i.span.lo;
                let codemap::BytePos(point2) = i.span.hi;
                debug!("PHIL name point is {} {}",point,point2);

                for &variant in enum_definition.variants.iter() {
                    let codemap::BytePos(point) = variant.span.lo;
                    self.values.push((String::from_str(token::get_ident(variant.node.name).get()), point as uint));
                }
            },
            _ => {}
        }
    }
}


pub fn parse_view_item(s: String) -> ViewItemVisitor {
    // parser can fail!() so isolate it in another task
    let result = task::try(proc() { 
        let cr = string_to_crate(s);
        let mut v = ViewItemVisitor{ident: None, paths: Vec::new()};
        visit::walk_crate(&mut v, &cr, ());
        return v;
    });
    match result {
        Ok(s) => {return s;},
        Err(_) => {
            return ViewItemVisitor{ident: None, paths: Vec::new()};
        }
    }
}

pub fn parse_let(s: String, fpath: Path, pos: uint, need_type: bool) -> Option<LetResult> {

    let result = task::try(proc() { 
        debug!("PHIL parse_let s=|{}| need_type={}",s, need_type);
        let stmt = string_to_stmt(s);
        debug!("PHIL parse_let stmt={:?}",stmt);
        let scope = Scope{filepath: fpath, point: pos};

        let mut v = LetVisitor{ scope: scope, result: None, need_type: need_type};
        visit::walk_stmt(&mut v, &*stmt, ());
        return v.result;
    });
    match result {
        Ok(s) => {return s;},
        Err(_) => {
            return None;
        }
    }
}

pub fn parse_struct_fields(s: String) -> Vec<(String, uint, Vec<String>)> {
    return task::try(proc() {
        let stmt = string_to_stmt(s);
        let mut v = StructVisitor{ fields: Vec::new() };
        visit::walk_stmt(&mut v, &*stmt, ());
        return v.fields;
    }).ok().unwrap_or(Vec::new());
}

pub fn parse_impl(s: String) -> ImplVisitor {
    return task::try(proc() {
        let stmt = string_to_stmt(s);
        let mut v = ImplVisitor { name_path: Vec::new(), trait_path: Vec::new() };
        visit::walk_stmt(&mut v, &*stmt, ());
        return v;
    }).ok().unwrap();    
}

pub fn parse_trait(s: String) -> TraitVisitor {
    return task::try(proc() {
        let stmt = string_to_stmt(s);
        let mut v = TraitVisitor { name: None };
        visit::walk_stmt(&mut v, &*stmt, ());
        return v;
    }).ok().unwrap();    
}

pub fn parse_type(s: String) -> TypeVisitor {
    return task::try(proc() {
        let stmt = string_to_stmt(s);
        let mut v = TypeVisitor { name: None, type_: Vec::new() };
        visit::walk_stmt(&mut v, &*stmt, ());
        return v;
    }).ok().unwrap();    
}


pub fn parse_fn_output(s: String) -> Vec<String> {
    return task::try(proc() {
        let stmt = string_to_stmt(s);
        let mut v = FnVisitor { name: "".to_string(), args: Vec::new(), output: Vec::new(), is_method: false };
        visit::walk_stmt(&mut v, &*stmt, ());
        return v.output;
    }).ok().unwrap();
}

pub fn parse_fn(s: String) -> FnVisitor {
    debug!("PHIL parse_fn |{}|",s);
    return task::try(proc() {
        let stmt = string_to_stmt(s);
        let mut v = FnVisitor { name: "".to_string(), args: Vec::new(), output: Vec::new(), is_method: false };
        visit::walk_stmt(&mut v, &*stmt, ());
        return v;
    }).ok().unwrap();
}

pub fn parse_mod(s: String) -> ModVisitor {
    return task::try(proc() {
        let stmt = string_to_stmt(s);
        let mut v = ModVisitor { name: None };
        visit::walk_stmt(&mut v, &*stmt, ());
        return v;
    }).ok().unwrap();    
}

pub fn parse_enum(s: String) -> EnumVisitor {
    return task::try(proc() {
        let stmt = string_to_stmt(s);
        let mut v = EnumVisitor { name: String::new(), values: Vec::new()};
        visit::walk_stmt(&mut v, &*stmt, ());
        return v;
    }).ok().unwrap();
}


pub fn get_type_of(s: String, fpath: &Path, pos: uint) -> Option<Match> {
    let myfpath = fpath.clone();

    return task::try(proc() {
        let stmt = string_to_stmt(s);
        let startscope = Scope {
            filepath: myfpath,
            point: pos
        };

        let mut v = ExprTypeVisitor{ scope: startscope,
                                     result: None};
        visit::walk_stmt(&mut v, &*stmt, ());
        return v.result;
    }).ok().unwrap();
}


#[test]
fn ast_sandbox() {

    //parse_let("let l : Vec<Blah>;".to_string(), Path::new("./ast.rs"), 0, true);
    //parse_let("let l = Vec<Blah>::new();".to_string(), Path::new("./ast.rs"), 0, true);

    //get_type_of("let l : Vec<Blah>;".to_string(), &Path::new("./ast.rs"), 0);
    //fail!();
    // let stmt = string_to_stmt(String::from_str(src));
    // let mut v = LetVisitor{ scope: Scope {filepath: Path::new("./foo"), point: 0} , result: None, parseinit: true};
    // visit::walk_stmt(&mut v, &*stmt, ());

    // println!("PHIL {:?}", stmt);
    // fail!();
    // println!("PHIL {}", v.name);
    // fail!("");
    // let mut v = ExprTypeVisitor{ scope: startscope,
    //                              result: None};
    

    // let src = ~"fn myfn(a: uint) -> Foo {}";
    // let src = ~"impl blah{    fn visit_item(&mut self, item: &ast::Item, _: ()) {} }";

    // let src = "Foo::Bar().baz(32)";
    //let src = "std::vec::Vec::new().push_all()";
    // let src = "impl visit::Visitor<()> for ExprTypeVisitor {}";
    // let src = "impl<'a> StrSlice<'a> for &'a str {}";

    //let src = "a(|b|{});";
    // let src = "(a,b) = (3,4);";
    // let src = "fn foo() -> (a,b) {}";

    // let stmt = string_to_stmt(String::from_str(src));

    // let src = "extern crate core_collections = \"collections\";";
    // let src = "use foo = bar;";
    // let src = "use bar::baz;";

    // let src = "use bar::{baz,blah};";
    // let src = "extern crate collections;";
    // let cr = string_to_crate(String::from_str(src));
    // let mut v = ViewItemVisitor{ ident: None, paths: Vec::new() };
    // visit::walk_crate(&mut v, &cr, ());

    // println!("PHIL v {} {}",v.ident, v.paths);

    //visit::walk_stmt(&mut v, stmt, ());

    // println!("PHIL stmt {:?}",stmt);


    // let mut v = ImplVisitor{ name_path: Vec::new() };
    // visit::walk_stmt(&mut v, stmt, ());


    // println!("v {}",v.name_path);

// pub struct Match {
//     pub matchstr: ~str,
//     pub filepath: Path,
//     pub point: uint,
//     pub linetxt: ~str,
//     pub local: bool,
//     pub mtype: MatchType
// }

    // let startscope = Match{ 
    //     matchstr: "".to_owned(),
    //     filepath: Path::new("./ast.rs"),
    //     point: 0,
    //     linetxt: "".to_owned(),
    //     local: true,
    //     mtype: racer::Module
    // };

    // let startscope = Scope {
    //     filepath: Path::new("./ast.rs"),
    //     point: 0
    // };

    // let mut v = ExprTypeVisitor{ scope: startscope,
    //                              result: None};
    // visit::walk_stmt(&mut v, stmt, ());

    // println!("PHIL result was {:?}",v.result);
    //return v.result;

    // let mut v = EnumVisitor { name: String::new(), values: Vec::new()};
    // visit::walk_stmt(&mut v, cr, ());
    // println!("PHIL {} {}", v.name, v.values);

    // let src = "let v = Foo::new();".to_owned();

    // let res = parse_let(src);
    // debug!("PHIL res {}",res.unwrap().init);
}

