#![no_std]
#![no_main]
#![feature(abi_efiapi)]
use core::panic::PanicInfo;
use core::mem::offset_of;
type H=*const core::ffi::c_void;
#[repr(C)]struct G{d1:u32,d2:u16,d3:u16,d4:[u8;8]}
#[repr(C)]struct Hdr{s:u64,r:u32,h:u32,c:u32,rsv:u32}
#[repr(C)]struct TO{r:usize,out:unsafe extern "efiapi" fn(*const TO,*const u16)->usize,
    t:usize,q:usize,s:usize,a:usize,cls:unsafe extern "efiapi" fn(*const TO)->usize,sc:usize,ec:usize,m:usize}
#[repr(C)]struct Gp{q:usize,s:usize,b:usize,md:*const GM}
#[repr(C)]struct GI{ve:u32,w:u32,h:u32,f:u32,p:u32,s:u32}
#[repr(C)]struct GM{mx:u32,mo:u32,i:*const GI,sz:u64,fb:u64,fs:u64}
#[repr(C)]struct BS{hdr:Hdr,f:[usize;26],eb:unsafe extern "efiapi" fn(H,usize)->usize,g:[usize;10],lp:unsafe extern "efiapi" fn(*const G,H,*mut H)->usize,tail:[usize;5]}
#[repr(C)]struct ST{h:Hdr,fw:*const u16,fr:u32,pad1:u32,cih:H,ci:H,coh:H,co:*const TO,seh:H,se:H,rt:H,bs:*const BS}
#[repr(C)]struct BI{m:u32,fa:u64,fsz:u64,fw:u32,fh:u32,fs:u32,ff:u32}
const GG:G=G{d1:0x9042a9de,d2:0x23dc,d3:0x4a38,d4:[0x96,0xfb,0x7a,0xde,0xd0,0x80,0x51,0x6a]};
const OK:usize=0;const BA:usize=0x10000;const KA:usize=0x100000;const S64O:usize=0x1D8;
static KERN:&[u8]=include_bytes!("../../../target/x86_64-unknown-none/release/karte-os-kernel");
const _:()={assert!(offset_of!(ST,co)==64);assert!(offset_of!(ST,bs)==96);
    assert!(offset_of!(TO,out)==8);assert!(offset_of!(TO,cls)==48);
    assert!(offset_of!(BS,lp)==320);assert!(offset_of!(BS,eb)==232);
    assert!(offset_of!(Gp,md)==24);assert!(offset_of!(GM,fb)==24);assert!(offset_of!(GM,i)==8);};
fn pr(co:&TO,s:&str){let mut b:[u16;64]=[0;64];let mut i=0;for c in s.bytes(){if i<62{b[i]=c as u16;i+=1;}if c==b'\n'&&i<62{b[i]=b'\r' as u16;i+=1;}}unsafe{(co.out)(co,b.as_ptr());}}

// Kernel entry point as a function pointer — PE allows direct function calls
type KernelEntry = unsafe extern "sysv64" fn(u32, usize) -> !;

#[unsafe(no_mangle)]
pub extern "efiapi" fn efi_main(h:H,st_p:*const ST)->!{
    let st=unsafe{&*st_p};let bs=unsafe{&*st.bs};let co=unsafe{&*st.co};
    unsafe{(co.cls)(co);}pr(co,"KOS v28\nGOP...");
    let bi=unsafe{&mut*(BA as*mut BI)};bi.m=0x474F5046;bi.fa=0;
    let mut g:H=core::ptr::null();
    if unsafe{(bs.lp)(&GG,core::ptr::null(),&mut g)}==OK&&!g.is_null(){
        let gp=unsafe{&*(g as*const Gp)};
        if !gp.md.is_null(){let m=unsafe{&*gp.md};
            if !m.i.is_null(){let i=unsafe{&*m.i};
                bi.fa=m.fb;bi.fsz=m.fs;bi.fw=i.w;bi.fh=i.h;bi.fs=i.s*4;bi.ff=1;
            }
        }
    }
    pr(co,"OK\nKERNEL...");
    unsafe{core::slice::from_raw_parts_mut(KA as*mut u8,KERN.len())}.copy_from_slice(KERN);
    pr(co,"OK\nEXIT...");
    unsafe{(bs.eb)(h,0);}
    pr(co,"OK\n");

    // Page tables: 1GB huge pages, 0-4GB + high-half
    unsafe{core::ptr::write_bytes(0x200000usize as*mut u8,0,4096*3);}
    unsafe{*(0x200000usize as*mut u64)=0x201003u64;}
    unsafe{*(0x200000usize as*mut u64).add(511)=0x202003u64;}
    for i in 0..4u64{unsafe{*(0x201000usize as*mut u64).add(i as usize)=(i<<30)|0x83;}}
    for i in 0..4u64{unsafe{*(0x202000usize as*mut u64).add(i as usize)=(i<<30)|0x83;}}
    unsafe{core::ptr::write_volatile(0x8000usize as*mut u32,0x36d76289u32);}

    pr(co,"BOOT...");

    // Direct function call — no asm, no jmp, no SEH issues
    // The kernel _start64 is at KA+S64O in the copied kernel binary
    let entry: KernelEntry = unsafe { core::mem::transmute(KA + S64O) };
    unsafe { entry(0x36d76289, 0); }
}
#[panic_handler]fn ph(_:&PanicInfo)->!{loop{}}
